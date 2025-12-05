use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ItemMod;

use super::autogen;
use super::parse::{ParsedArg, ParsedMethod, ParsedModule, ParsedStruct};

pub fn generate(parsed: &ParsedModule, original: &ItemMod) -> syn::Result<TokenStream> {
    let mod_name = &parsed.name;
    let mod_vis = &original.vis;
    let mod_attrs = &original.attrs;

    let original_items = original
        .content
        .as_ref()
        .map(|(_, items)| items.clone())
        .unwrap_or_default();

    let struct_impls: Vec<TokenStream> = parsed
        .structs
        .iter()
        .map(|s| generate_struct_impl(s, parsed))
        .collect::<syn::Result<_>>()?;

    let impl_registrations: Vec<TokenStream> = parsed
        .impls
        .iter()
        .map(|i| generate_impl_registration(i, parsed))
        .collect::<syn::Result<_>>()?;

    let schema_struct = generate_schema_struct(parsed)?;

    let sdl_generation = if parsed.args.generate.is_some() {
        autogen::generate_sdl_code(parsed)?
    } else {
        quote! {}
    };

    Ok(quote! {
        #(#mod_attrs)*
        #mod_vis mod #mod_name {
            #(#original_items)*

            #(#struct_impls)*

            #(#impl_registrations)*

            #schema_struct

            #sdl_generation
        }
    })
}

fn generate_struct_impl(s: &ParsedStruct, _module: &ParsedModule) -> syn::Result<TokenStream> {
    let name = &s.name;
    let graphql_name = &s.graphql_name;
    let is_query = s.is_query;
    let is_mutation = s.is_mutation;
    let is_subscription = s.is_subscription;

    let to_const_value = generate_to_const_value(s);
    let from_const_value = generate_from_const_value(s);

    let default_impl = if s.fields.is_empty() {
        quote! {
            impl ::std::default::Default for #name {
                fn default() -> Self {
                    Self
                }
            }
        }
    } else {
        quote! {}
    };

    let list_field_types: Vec<&str> = s
        .fields
        .iter()
        .filter_map(|f| {
            if f.is_list {
                f.inner_type.as_deref()
            } else {
                None
            }
        })
        .collect();

    Ok(quote! {
        #to_const_value

        #from_const_value

        #default_impl

        impl #name {
            #[doc(hidden)]
            pub const fn __graphql_type_name() -> &'static str {
                #graphql_name
            }

            #[doc(hidden)]
            pub const fn __is_query() -> bool {
                #is_query
            }

            #[doc(hidden)]
            pub const fn __is_mutation() -> bool {
                #is_mutation
            }

            #[doc(hidden)]
            pub const fn __is_subscription() -> bool {
                #is_subscription
            }

            #[doc(hidden)]
            pub fn __list_field_types() -> &'static [&'static str] {
                &[#(#list_field_types),*]
            }
        }

        impl ::convoy_graphql::__private::GraphQLObjectInfo for #name {
            const TYPE_NAME: &'static str = #graphql_name;
            const IS_QUERY: bool = #is_query;
            const IS_MUTATION: bool = #is_mutation;

            fn list_field_types() -> &'static [&'static str] {
                &[#(#list_field_types),*]
            }
        }
    })
}

fn generate_to_const_value(s: &ParsedStruct) -> TokenStream {
    let name = &s.name;

    if s.fields.is_empty() {
        return quote! {
            impl ::convoy_graphql::ToConstValue for #name {
                fn to_const_value(&self) -> ::convoy_graphql::ConstValue {
                    ::convoy_graphql::ConstValue::Object(::indexmap::IndexMap::new())
                }
            }
        };
    }

    let field_conversions: Vec<_> = s
        .fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let field_name_str = field_name.to_string();
            quote! {
                ::async_graphql::Name::new(#field_name_str) => self.#field_name.to_const_value()
            }
        })
        .collect();

    quote! {
        impl ::convoy_graphql::ToConstValue for #name {
            fn to_const_value(&self) -> ::convoy_graphql::ConstValue {
                use ::convoy_graphql::ToConstValue;
                ::convoy_graphql::ConstValue::Object(::indexmap::indexmap! {
                    #(#field_conversions),*
                })
            }
        }
    }
}

fn generate_from_const_value(s: &ParsedStruct) -> TokenStream {
    let name = &s.name;

    if s.fields.is_empty() {
        return quote! {
            impl ::convoy_graphql::FromConstValue for #name {
                fn from_const_value(_value: &::convoy_graphql::ConstValue) -> Result<Self, String> {
                    Ok(Self)
                }
            }
        };
    }

    let field_extractions: Vec<_> = s
        .fields
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let field_name_str = field_name.to_string();
            quote! {
                #field_name: {
                    let val = obj.get(#field_name_str)
                        .ok_or_else(|| format!("missing field: {}", #field_name_str))?;
                    ::convoy_graphql::FromConstValue::from_const_value(val)?
                }
            }
        })
        .collect();

    quote! {
        impl ::convoy_graphql::FromConstValue for #name {
            fn from_const_value(value: &::convoy_graphql::ConstValue) -> Result<Self, String> {
                match value {
                    ::convoy_graphql::ConstValue::Object(obj) => {
                        Ok(Self {
                            #(#field_extractions),*
                        })
                    }
                    _ => Err("expected object".to_string()),
                }
            }
        }
    }
}

fn generate_impl_registration(
    impl_block: &super::parse::ParsedImpl,
    module: &ParsedModule,
) -> syn::Result<TokenStream> {
    let type_name = &impl_block.type_name;

    let is_subscription_type = module
        .subscription_type()
        .map(|s| &s.name == type_name)
        .unwrap_or(false);

    let resolver_metas: Vec<_> = impl_block
        .methods
        .iter()
        .map(|m| {
            let name = m.name.to_string();
            let is_batched = m.batch_config.is_some();
            let batch_key = m
                .batch_config
                .as_ref()
                .map(|b| {
                    let k = &b.key;
                    quote! { Some(#k) }
                })
                .unwrap_or_else(|| quote! { None });
            let delay_ms = m.batch_config.as_ref().map(|b| b.delay_ms).unwrap_or(0);
            let is_list_return = m.is_list_return;
            let inner_type = m
                .inner_return_type
                .as_ref()
                .map(|t| quote! { Some(#t) })
                .unwrap_or_else(|| quote! { None });

            quote! {
                ::convoy_graphql::__private::ResolverMeta {
                    name: #name,
                    is_batched: #is_batched,
                    batch_key: #batch_key,
                    batch_delay_ms: #delay_ms,
                    is_list_return: #is_list_return,
                    inner_return_type: #inner_type,
                }
            }
        })
        .collect();

    let list_return_types: Vec<_> = impl_block
        .methods
        .iter()
        .filter_map(|r| {
            if r.is_list_return {
                r.inner_return_type.as_deref()
            } else {
                None
            }
        })
        .collect();

    let field_registrations: Vec<_> = if !is_subscription_type {
        impl_block
            .methods
            .iter()
            .map(|m| generate_field_registration(type_name, m))
            .collect::<syn::Result<_>>()?
    } else {
        vec![]
    };

    let subscription_registrations: Vec<_> = if is_subscription_type {
        impl_block
            .methods
            .iter()
            .map(|m| generate_subscription_field_registration(type_name, m))
            .collect::<syn::Result<_>>()?
    } else {
        vec![]
    };

    let all_batched = impl_block.methods.iter().all(|r| r.batch_config.is_some());
    let batch_enabled_impl = if all_batched {
        quote! {
            impl ::convoy_graphql::__private::BatchEnabled for #type_name {}
        }
    } else {
        quote! {}
    };

    let meta_struct_name = format_ident!("__{}ResolverMeta", type_name);

    Ok(quote! {
        #[doc(hidden)]
        pub struct #meta_struct_name;

        impl #meta_struct_name {
            pub fn resolvers() -> Vec<::convoy_graphql::__private::ResolverMeta> {
                vec![#(#resolver_metas),*]
            }
        }

        impl ::convoy_graphql::__private::ResolverMetadata for #type_name {
            fn resolver_meta() -> Vec<::convoy_graphql::__private::ResolverMeta> {
                #meta_struct_name::resolvers()
            }
        }

        #batch_enabled_impl

        impl #type_name {
            #[doc(hidden)]
            pub fn __register_graphql_fields(
                obj: ::async_graphql::dynamic::Object
            ) -> ::async_graphql::dynamic::Object {
                use ::async_graphql::dynamic::{Field, FieldFuture, FieldValue, TypeRef, InputValue};
                use ::convoy_graphql::{Ctx, ToConstValue, FromConstValue, RequestMetadata};

                fn const_value_to_field_value(value: ::convoy_graphql::ConstValue) -> FieldValue<'static> {
                    match value {
                        ::convoy_graphql::ConstValue::List(items) => {
                            let field_values: Vec<FieldValue<'static>> = items
                                .into_iter()
                                .map(const_value_to_field_value)
                                .collect();
                            FieldValue::list(field_values)
                        }
                        ::convoy_graphql::ConstValue::Object(_) => {
                            FieldValue::owned_any(value)
                        }
                        other => FieldValue::from(other),
                    }
                }

                let obj = obj #(#field_registrations)*;
                obj
            }

            #[doc(hidden)]
            pub fn __list_return_types() -> &'static [&'static str] {
                &[#(#list_return_types),*]
            }

            #[doc(hidden)]
            pub fn __register_graphql_subscriptions(
                sub: ::async_graphql::dynamic::Subscription
            ) -> ::async_graphql::dynamic::Subscription {
                use ::async_graphql::dynamic::{SubscriptionField, SubscriptionFieldFuture, FieldValue, TypeRef, InputValue};
                use ::convoy_graphql::{Ctx, ToConstValue, FromConstValue, RequestMetadata};
                use ::futures_util::StreamExt;

                fn const_value_to_field_value(value: ::convoy_graphql::ConstValue) -> FieldValue<'static> {
                    match value {
                        ::convoy_graphql::ConstValue::List(items) => {
                            let field_values: Vec<FieldValue<'static>> = items
                                .into_iter()
                                .map(const_value_to_field_value)
                                .collect();
                            FieldValue::list(field_values)
                        }
                        ::convoy_graphql::ConstValue::Object(_) => {
                            FieldValue::owned_any(value)
                        }
                        other => FieldValue::from(other),
                    }
                }

                let sub = sub #(#subscription_registrations)*;
                sub
            }
        }
    })
}

fn generate_field_registration(
    type_name: &syn::Ident,
    method: &ParsedMethod,
) -> syn::Result<TokenStream> {
    let field_name = method.name.to_string();
    let method_name = &method.name;

    let graphql_type = rust_type_to_graphql_type(&method.return_type);

    let arg_defs: Vec<_> = method
        .args
        .iter()
        .map(|arg| {
            let arg_name = arg.name.to_string();
            let arg_type = rust_type_to_graphql_type(&arg.ty);
            quote! {
                .argument(InputValue::new(#arg_name, #arg_type))
            }
        })
        .collect();

    let arg_extractions: Vec<_> = method.args.iter().map(generate_arg_extraction).collect();

    let arg_names: Vec<_> = method.args.iter().map(|a| &a.name).collect();

    Ok(quote! {
        .field(Field::new(#field_name, #graphql_type, |ctx| {
            FieldFuture::new(async move {
                let metadata = RequestMetadata::default();
                let args = ctx.args.as_index_map();
                let parent_val = ctx.parent_value.downcast_ref::<::convoy_graphql::ConstValue>();
                let ctx_wrapper = Ctx::new(parent_val, Some(&args), &metadata);

                #(#arg_extractions)*

                let empty_obj = ::convoy_graphql::ConstValue::Object(::indexmap::IndexMap::new());
                let parent = ctx.parent_value.downcast_ref::<::convoy_graphql::ConstValue>()
                    .unwrap_or(&empty_obj);
                let instance = #type_name::from_const_value(parent)
                    .map_err(|e| ::async_graphql::Error::new(e))?;

                let result = instance.#method_name(&ctx_wrapper, #(#arg_names),*).await;

                match result {
                    Ok(value) => {
                        let const_val = value.to_const_value();
                        Ok(Some(const_value_to_field_value(const_val)))
                    }
                    Err(e) => Err(::async_graphql::Error::new(format!("{}", e))),
                }
            })
        }) #(#arg_defs)*)
    })
}

fn generate_arg_extraction(arg: &ParsedArg) -> TokenStream {
    let arg_name = &arg.name;
    let arg_name_str = arg_name.to_string();
    let arg_ty = &arg.ty;

    quote! {
        let #arg_name: #arg_ty = ctx_wrapper.arg_as(#arg_name_str)
            .ok_or_else(|| ::async_graphql::Error::new(
                format!("missing required argument: {}", #arg_name_str)
            ))?;
    }
}

fn generate_subscription_field_registration(
    _type_name: &syn::Ident,
    method: &ParsedMethod,
) -> syn::Result<TokenStream> {
    let field_name = method.name.to_string();
    let method_name = &method.name;

    let graphql_type = extract_subscription_item_type(&method.return_type);

    let arg_defs: Vec<_> = method
        .args
        .iter()
        .map(|arg| {
            let arg_name = arg.name.to_string();
            let arg_type = rust_type_to_graphql_type(&arg.ty);
            quote! {
                .argument(InputValue::new(#arg_name, #arg_type))
            }
        })
        .collect();

    Ok(quote! {
        .field(SubscriptionField::new(#field_name, #graphql_type, |ctx| {
            SubscriptionFieldFuture::new(async move {
                let metadata = RequestMetadata::default();
                let args = ctx.args.as_index_map();
                let ctx_wrapper = Ctx::new(None, Some(&args), &metadata);

                let instance = Subscription::default();

                let stream = instance.#method_name(&ctx_wrapper).await;

                let mapped_stream = stream.map(|result| {
                    match result {
                        Ok(value) => {
                            let const_val = value.to_const_value();
                            Ok(const_value_to_field_value(const_val))
                        }
                        Err(e) => Err(::async_graphql::Error::new(format!("{}", e))),
                    }
                });

                Ok(mapped_stream)
            })
        }) #(#arg_defs)*)
    })
}

fn extract_subscription_item_type(ty: &syn::Type) -> TokenStream {
    if let syn::Type::Path(path) = ty {
        if let Some(segment) = path.path.segments.last() {
            let type_name = segment.ident.to_string();

            if type_name == "Pin" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return extract_subscription_item_type(inner);
                    }
                }
            }

            if type_name == "Box" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return extract_subscription_item_type(inner);
                    }
                }
            }

            if type_name == "Result" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return rust_type_to_graphql_type(inner);
                    }
                }
            }
        }
    }

    if let syn::Type::TraitObject(trait_obj) = ty {
        for bound in &trait_obj.bounds {
            if let syn::TypeParamBound::Trait(trait_bound) = bound {
                if let Some(segment) = trait_bound.path.segments.last() {
                    if segment.ident == "Stream" {
                        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                            for arg in &args.args {
                                if let syn::GenericArgument::AssocType(assoc) = arg {
                                    if assoc.ident == "Item" {
                                        return extract_subscription_item_type(&assoc.ty);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    rust_type_to_graphql_type(ty)
}

fn generate_schema_struct(parsed: &ParsedModule) -> syn::Result<TokenStream> {
    let query_type = parsed.query_type().ok_or_else(|| {
        syn::Error::new(
            parsed.name.span(),
            "GraphQLSchema module must contain a Query type (struct named `Query` or marked with #[query])",
        )
    })?;

    let query_type_name = &query_type.name;
    let query_graphql_name = &query_type.graphql_name;

    let mutation_type_name_setup = if parsed.mutation_type().is_some() {
        let mutation_graphql_name = &parsed.mutation_type().unwrap().graphql_name;
        quote! {
            let mutation_type_name = Some(#mutation_graphql_name);
        }
    } else {
        quote! {
            let mutation_type_name: Option<&str> = None;
        }
    };

    let mutation_registration = if let Some(mutation) = parsed.mutation_type() {
        let mutation_type_name = &mutation.name;
        let mutation_graphql_name = &mutation.graphql_name;
        quote! {
            {
                let mut obj = ::async_graphql::dynamic::Object::new(#mutation_graphql_name);
                obj = #mutation_type_name::__register_graphql_fields(obj);
                builder = builder.register(obj);
            }
        }
    } else {
        quote! {}
    };

    let subscription_type_name_setup = if parsed.subscription_type().is_some() {
        let subscription_graphql_name = &parsed.subscription_type().unwrap().graphql_name;
        quote! {
            let subscription_type_name = Some(#subscription_graphql_name);
        }
    } else {
        quote! {
            let subscription_type_name: Option<&str> = None;
        }
    };

    let subscription_registration = if let Some(subscription) = parsed.subscription_type() {
        let subscription_type_name = &subscription.name;
        let subscription_graphql_name = &subscription.graphql_name;
        quote! {
            {
                let mut sub = ::async_graphql::dynamic::Subscription::new(#subscription_graphql_name);
                sub = #subscription_type_name::__register_graphql_subscriptions(sub);
                builder = builder.register(sub);
            }
        }
    } else {
        quote! {}
    };

    let type_registrations: Vec<_> = parsed
        .structs
        .iter()
        .filter(|s| !s.is_query && !s.is_mutation && !s.is_subscription)
        .map(|s| {
            let ty = &s.name;
            let graphql_name = &s.graphql_name;
            quote! {
                {
                    let mut obj = ::async_graphql::dynamic::Object::new(#graphql_name);
                    obj = #ty::__register_graphql_fields(obj);
                    builder = builder.register(obj);
                }
            }
        })
        .collect();

    Ok(quote! {
        pub struct Schema {
            inner: ::convoy_graphql::BuiltSchema,
        }

        impl Schema {
            pub fn build() -> ::std::result::Result<Self, ::convoy_graphql::SchemaError> {
                use ::async_graphql::dynamic::{self, FieldFuture, FieldValue, TypeRef, InputValue};
                use ::convoy_graphql::{ToConstValue, FromConstValue, RequestMetadata, Ctx};

                #mutation_type_name_setup
                #subscription_type_name_setup

                let mut builder = dynamic::Schema::build(#query_graphql_name, mutation_type_name, subscription_type_name);

                {
                    let mut obj = ::async_graphql::dynamic::Object::new(#query_graphql_name);
                    obj = #query_type_name::__register_graphql_fields(obj);
                    builder = builder.register(obj);
                }

                #mutation_registration

                #subscription_registration

                #(#type_registrations)*

                let graphql_schema = builder.finish().map_err(|e| {
                    ::convoy_graphql::SchemaError::BuildError {
                        message: format!("Failed to build schema: {}", e),
                    }
                })?;

                Ok(Self {
                    inner: ::convoy_graphql::BuiltSchema::from_dynamic_schema(graphql_schema),
                })
            }

            pub async fn execute(&self, query: &str) -> ::async_graphql::Response {
                self.inner.execute_query(query).await
            }

            pub async fn execute_request(
                &self,
                request: ::async_graphql::Request,
            ) -> ::async_graphql::Response {
                self.inner.execute(request).await
            }

            pub fn inner(&self) -> &::convoy_graphql::BuiltSchema {
                &self.inner
            }
        }
    })
}

fn rust_type_to_graphql_type(ty: &syn::Type) -> TokenStream {
    match ty {
        syn::Type::Path(path) => {
            if let Some(segment) = path.path.segments.last() {
                let type_name = segment.ident.to_string();

                match type_name.as_str() {
                    "Result" => {
                        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                                return rust_type_to_graphql_type(inner);
                            }
                        }
                        quote! { TypeRef::Named("String".into()) }
                    }
                    "Option" => {
                        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                                return rust_type_to_graphql_type(inner);
                            }
                        }
                        quote! { TypeRef::Named("String".into()) }
                    }
                    "Vec" => {
                        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                                let inner_ref = rust_type_to_graphql_type_inner(inner);
                                return quote! {
                                    TypeRef::NonNull(Box::new(
                                        TypeRef::List(Box::new(
                                            TypeRef::NonNull(Box::new(#inner_ref))
                                        ))
                                    ))
                                };
                            }
                        }
                        quote! { TypeRef::List(Box::new(TypeRef::Named("String".into()))) }
                    }
                    "i32" | "i64" => {
                        quote! { TypeRef::NonNull(Box::new(TypeRef::Named("Int".into()))) }
                    }
                    "f32" | "f64" => {
                        quote! { TypeRef::NonNull(Box::new(TypeRef::Named("Float".into()))) }
                    }
                    "bool" => {
                        quote! { TypeRef::NonNull(Box::new(TypeRef::Named("Boolean".into()))) }
                    }
                    "String" => {
                        quote! { TypeRef::NonNull(Box::new(TypeRef::Named("String".into()))) }
                    }
                    other => {
                        quote! { TypeRef::NonNull(Box::new(TypeRef::Named(#other.into()))) }
                    }
                }
            } else {
                quote! { TypeRef::Named("String".into()) }
            }
        }
        _ => quote! { TypeRef::Named("String".into()) },
    }
}

fn rust_type_to_graphql_type_inner(ty: &syn::Type) -> TokenStream {
    match ty {
        syn::Type::Path(path) => {
            if let Some(segment) = path.path.segments.last() {
                let type_name = segment.ident.to_string();

                match type_name.as_str() {
                    "i32" | "i64" => quote! { TypeRef::Named("Int".into()) },
                    "f32" | "f64" => quote! { TypeRef::Named("Float".into()) },
                    "bool" => quote! { TypeRef::Named("Boolean".into()) },
                    "String" => quote! { TypeRef::Named("String".into()) },
                    other => quote! { TypeRef::Named(#other.into()) },
                }
            } else {
                quote! { TypeRef::Named("String".into()) }
            }
        }
        _ => quote! { TypeRef::Named("String".into()) },
    }
}
