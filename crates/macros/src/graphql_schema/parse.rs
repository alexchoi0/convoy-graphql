use proc_macro2::{Ident, TokenStream};
use syn::{
    parse::Parser, Attribute, FnArg, ImplItem, Item, ItemImpl, ItemMod, ItemStruct, Lit, Pat,
    ReturnType, Type,
};

#[derive(Debug, Default, Clone)]
pub struct MacroArgs {
    pub generate: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BatchConfig {
    pub key: String,
    pub delay_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ParsedField {
    pub name: Ident,
    pub ty: Type,
    pub is_list: bool,
    pub inner_type: Option<String>,
}

#[derive(Debug)]
pub struct ParsedStruct {
    pub name: Ident,
    pub graphql_name: String,
    pub is_query: bool,
    pub is_mutation: bool,
    pub is_subscription: bool,
    pub fields: Vec<ParsedField>,
}

#[derive(Debug, Clone)]
pub struct ParsedArg {
    pub name: Ident,
    pub ty: Type,
}

#[derive(Debug)]
pub struct ParsedMethod {
    pub name: Ident,
    pub args: Vec<ParsedArg>,
    pub return_type: Type,
    pub batch_config: Option<BatchConfig>,
    pub is_list_return: bool,
    pub inner_return_type: Option<String>,
}

#[derive(Debug)]
pub struct ParsedImpl {
    pub type_name: Ident,
    pub methods: Vec<ParsedMethod>,
}

#[derive(Debug)]
pub struct ParsedModule {
    pub name: Ident,
    pub args: MacroArgs,
    pub structs: Vec<ParsedStruct>,
    pub impls: Vec<ParsedImpl>,
}

impl ParsedModule {
    pub fn query_type(&self) -> Option<&ParsedStruct> {
        self.structs
            .iter()
            .find(|s| s.is_query || s.name == "Query")
    }

    pub fn mutation_type(&self) -> Option<&ParsedStruct> {
        self.structs
            .iter()
            .find(|s| s.is_mutation || s.name == "Mutation")
    }

    pub fn subscription_type(&self) -> Option<&ParsedStruct> {
        self.structs
            .iter()
            .find(|s| s.is_subscription || s.name == "Subscription")
    }

    pub fn impl_for(&self, type_name: &str) -> Option<&ParsedImpl> {
        self.impls.iter().find(|i| i.type_name == type_name)
    }
}

pub fn parse_macro_args(attr: TokenStream) -> syn::Result<MacroArgs> {
    let mut args = MacroArgs::default();

    if attr.is_empty() {
        return Ok(args);
    }

    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("generate") {
            let value: Lit = meta.value()?.parse()?;
            if let Lit::Str(s) = value {
                args.generate = Some(s.value());
            }
        }
        Ok(())
    });

    parser.parse2(attr)?;
    Ok(args)
}

pub fn parse_module(module: &ItemMod, args: MacroArgs) -> syn::Result<ParsedModule> {
    let name = module.ident.clone();

    let content = module.content.as_ref().ok_or_else(|| {
        syn::Error::new_spanned(
            module,
            "GraphQLSchema module must have inline content (use `mod name { ... }` not `mod name;`)",
        )
    })?;

    let mut structs = Vec::new();
    let mut impls = Vec::new();

    for item in &content.1 {
        match item {
            Item::Struct(s) => {
                structs.push(parse_struct(s)?);
            }
            Item::Impl(i) => {
                if i.trait_.is_none() {
                    impls.push(parse_impl(i)?);
                }
            }
            _ => {}
        }
    }

    Ok(ParsedModule {
        name,
        args,
        structs,
        impls,
    })
}

fn parse_struct(item: &ItemStruct) -> syn::Result<ParsedStruct> {
    let name = item.ident.clone();
    let (is_query, is_mutation, is_subscription, custom_name) = parse_struct_attrs(&item.attrs)?;

    let graphql_name = custom_name.unwrap_or_else(|| name.to_string());

    let is_query = is_query || name == "Query";
    let is_mutation = is_mutation || name == "Mutation";
    let is_subscription = is_subscription || name == "Subscription";

    let fields = parse_struct_fields(item)?;

    Ok(ParsedStruct {
        name,
        graphql_name,
        is_query,
        is_mutation,
        is_subscription,
        fields,
    })
}

fn parse_struct_attrs(attrs: &[Attribute]) -> syn::Result<(bool, bool, bool, Option<String>)> {
    let mut is_query = false;
    let mut is_mutation = false;
    let mut is_subscription = false;
    let mut custom_name = None;

    for attr in attrs {
        if attr.path().is_ident("query") {
            is_query = true;
        } else if attr.path().is_ident("mutation") {
            is_mutation = true;
        } else if attr.path().is_ident("subscription") {
            is_subscription = true;
        } else if attr.path().is_ident("graphql") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("query") {
                    is_query = true;
                } else if meta.path.is_ident("mutation") {
                    is_mutation = true;
                } else if meta.path.is_ident("subscription") {
                    is_subscription = true;
                } else if meta.path.is_ident("name") {
                    let value: Lit = meta.value()?.parse()?;
                    if let Lit::Str(s) = value {
                        custom_name = Some(s.value());
                    }
                }
                Ok(())
            })?;
        }
    }

    Ok((is_query, is_mutation, is_subscription, custom_name))
}

fn parse_struct_fields(item: &ItemStruct) -> syn::Result<Vec<ParsedField>> {
    let mut fields = Vec::new();

    if let syn::Fields::Named(named) = &item.fields {
        for field in &named.named {
            if let Some(name) = &field.ident {
                let (is_list, inner_type) = analyze_type(&field.ty);
                fields.push(ParsedField {
                    name: name.clone(),
                    ty: field.ty.clone(),
                    is_list,
                    inner_type,
                });
            }
        }
    }

    Ok(fields)
}

fn parse_impl(item: &ItemImpl) -> syn::Result<ParsedImpl> {
    let type_name = extract_type_name(&item.self_ty)?;

    let mut methods = Vec::new();

    for impl_item in &item.items {
        if let ImplItem::Fn(method) = impl_item {
            if let Some(parsed) = parse_method(method)? {
                methods.push(parsed);
            }
        }
    }

    Ok(ParsedImpl { type_name, methods })
}

fn extract_type_name(ty: &Type) -> syn::Result<Ident> {
    match ty {
        Type::Path(path) => {
            if let Some(segment) = path.path.segments.last() {
                Ok(segment.ident.clone())
            } else {
                Err(syn::Error::new_spanned(ty, "Expected type path"))
            }
        }
        _ => Err(syn::Error::new_spanned(ty, "Expected type path")),
    }
}

fn parse_method(method: &syn::ImplItemFn) -> syn::Result<Option<ParsedMethod>> {
    let name = method.sig.ident.clone();

    if name.to_string().starts_with('_') {
        return Ok(None);
    }

    let batch_config = parse_batch_attr(&method.attrs)?;

    let args = parse_method_args(&method.sig.inputs)?;

    let return_type = match &method.sig.output {
        ReturnType::Type(_, ty) => *ty.clone(),
        ReturnType::Default => {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "Resolver must have a return type",
            ));
        }
    };

    let (is_list_return, inner_return_type) = analyze_return_type(&return_type);

    Ok(Some(ParsedMethod {
        name,
        args,
        return_type,
        batch_config,
        is_list_return,
        inner_return_type,
    }))
}

fn parse_batch_attr(attrs: &[Attribute]) -> syn::Result<Option<BatchConfig>> {
    for attr in attrs {
        if attr.path().is_ident("batch") {
            let mut key = None;
            let mut delay_ms = 1u64;

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("key") {
                    let value: Lit = meta.value()?.parse()?;
                    if let Lit::Str(s) = value {
                        key = Some(s.value());
                    }
                } else if meta.path.is_ident("delay_ms") {
                    let value: Lit = meta.value()?.parse()?;
                    if let Lit::Int(i) = value {
                        delay_ms = i.base10_parse()?;
                    }
                }
                Ok(())
            })?;

            let key = key.ok_or_else(|| {
                syn::Error::new_spanned(
                    attr,
                    "batch attribute requires `key` parameter: #[batch(key = \"field\")]",
                )
            })?;

            return Ok(Some(BatchConfig { key, delay_ms }));
        }
    }
    Ok(None)
}

fn parse_method_args(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> syn::Result<Vec<ParsedArg>> {
    let mut args = Vec::new();

    for input in inputs {
        match input {
            FnArg::Receiver(_) => continue,
            FnArg::Typed(pat_type) => {
                if is_ctx_type(&pat_type.ty) {
                    continue;
                }

                let name = match &*pat_type.pat {
                    Pat::Ident(pat_ident) => pat_ident.ident.clone(),
                    _ => continue,
                };

                args.push(ParsedArg {
                    name,
                    ty: (*pat_type.ty).clone(),
                });
            }
        }
    }

    Ok(args)
}

fn is_ctx_type(ty: &Type) -> bool {
    match ty {
        Type::Reference(r) => is_ctx_type(&r.elem),
        Type::Path(path) => {
            if let Some(segment) = path.path.segments.last() {
                segment.ident == "Ctx"
            } else {
                false
            }
        }
        _ => false,
    }
}

fn analyze_type(ty: &Type) -> (bool, Option<String>) {
    if let Type::Path(path) = ty {
        if let Some(segment) = path.path.segments.last() {
            let type_name = segment.ident.to_string();

            if type_name == "Vec" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return (true, Some(extract_inner_type_name(inner)));
                    }
                }
                return (true, None);
            }

            if type_name == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return analyze_type(inner);
                    }
                }
            }

            if type_name == "Result" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return analyze_type(inner);
                    }
                }
            }
        }
    }

    (false, None)
}

fn analyze_return_type(ty: &Type) -> (bool, Option<String>) {
    analyze_type(ty)
}

fn extract_inner_type_name(ty: &Type) -> String {
    match ty {
        Type::Path(path) => {
            if let Some(segment) = path.path.segments.last() {
                let name = segment.ident.to_string();

                if name == "Option" || name == "Result" || name == "Box" {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                            return extract_inner_type_name(inner);
                        }
                    }
                }

                name
            } else {
                "Unknown".to_string()
            }
        }
        _ => "Unknown".to_string(),
    }
}
