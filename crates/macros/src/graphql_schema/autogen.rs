use proc_macro2::TokenStream;
use quote::quote;

use super::parse::{ParsedMethod, ParsedModule, ParsedStruct};

pub fn generate_sdl_code(parsed: &ParsedModule) -> syn::Result<TokenStream> {
    let filename = parsed
        .args
        .generate
        .as_ref()
        .expect("generate_sdl_code called without generate argument");

    let sdl = generate_sdl_string(parsed);

    Ok(quote! {
        pub const SCHEMA_SDL: &str = #sdl;

        pub fn schema_sdl() -> &'static str {
            SCHEMA_SDL
        }

        pub fn write_schema_file() -> ::std::io::Result<()> {
            ::std::fs::write(#filename, SCHEMA_SDL)
        }
    })
}

fn generate_sdl_string(parsed: &ParsedModule) -> String {
    let mut sdl = String::new();

    sdl.push_str("schema {\n");
    if parsed.query_type().is_some() {
        sdl.push_str("  query: Query\n");
    }
    if parsed.mutation_type().is_some() {
        sdl.push_str("  mutation: Mutation\n");
    }
    if parsed.subscription_type().is_some() {
        sdl.push_str("  subscription: Subscription\n");
    }
    sdl.push_str("}\n\n");

    for s in &parsed.structs {
        sdl.push_str(&generate_type_sdl(s, parsed));
        sdl.push('\n');
    }

    sdl
}

fn generate_type_sdl(s: &ParsedStruct, module: &ParsedModule) -> String {
    use std::collections::HashSet;

    let mut sdl = format!("type {} {{\n", s.graphql_name);

    let mut added_fields: HashSet<String> = HashSet::new();

    for field in &s.fields {
        let field_name = field.name.to_string();
        if !added_fields.contains(&field_name) {
            let graphql_type = rust_type_to_sdl_type(&field.ty);
            sdl.push_str(&format!("  {}: {}\n", field_name, graphql_type));
            added_fields.insert(field_name);
        }
    }

    if let Some(impl_block) = module.impl_for(&s.name.to_string()) {
        for method in &impl_block.methods {
            let method_name = method.name.to_string();
            if !added_fields.contains(&method_name) {
                sdl.push_str(&generate_field_sdl(method));
                added_fields.insert(method_name);
            }
        }
    }

    sdl.push_str("}\n");
    sdl
}

fn generate_field_sdl(method: &ParsedMethod) -> String {
    let mut field = format!("  {}", method.name);

    if !method.args.is_empty() {
        let args: Vec<String> = method
            .args
            .iter()
            .map(|arg| {
                let arg_type = rust_type_to_sdl_type(&arg.ty);
                format!("{}: {}", arg.name, arg_type)
            })
            .collect();
        field.push_str(&format!("({})", args.join(", ")));
    }

    let return_type = rust_type_to_sdl_type(&method.return_type);
    field.push_str(&format!(": {}\n", return_type));

    field
}

fn rust_type_to_sdl_type(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(path) => {
            if let Some(segment) = path.path.segments.last() {
                let type_name = segment.ident.to_string();

                match type_name.as_str() {
                    "Result" => {
                        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                                return rust_type_to_sdl_type(inner);
                            }
                        }
                        "String".to_string()
                    }
                    "Option" => {
                        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                                let inner_type = rust_type_to_sdl_type_inner(inner);
                                return inner_type;
                            }
                        }
                        "String".to_string()
                    }
                    "Vec" => {
                        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                                let inner_type = rust_type_to_sdl_type_inner(inner);
                                return format!("[{}!]!", inner_type);
                            }
                        }
                        "[String]!".to_string()
                    }
                    "Pin" => {
                        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                                return extract_stream_item_type_sdl(inner);
                            }
                        }
                        "String!".to_string()
                    }
                    "Box" => {
                        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                                return extract_stream_item_type_sdl(inner);
                            }
                        }
                        "String!".to_string()
                    }
                    "i32" | "i64" => "Int!".to_string(),
                    "f32" | "f64" => "Float!".to_string(),
                    "bool" => "Boolean!".to_string(),
                    "String" => "String!".to_string(),
                    other => format!("{}!", other),
                }
            } else {
                "String".to_string()
            }
        }
        syn::Type::TraitObject(trait_obj) => extract_stream_item_type_from_trait_sdl(trait_obj),
        _ => "String".to_string(),
    }
}

fn extract_stream_item_type_sdl(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(path) => {
            if let Some(segment) = path.path.segments.last() {
                let type_name = segment.ident.to_string();
                if type_name == "Box" {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                            return extract_stream_item_type_sdl(inner);
                        }
                    }
                }
            }
            "String!".to_string()
        }
        syn::Type::TraitObject(trait_obj) => extract_stream_item_type_from_trait_sdl(trait_obj),
        _ => "String!".to_string(),
    }
}

fn extract_stream_item_type_from_trait_sdl(trait_obj: &syn::TypeTraitObject) -> String {
    for bound in &trait_obj.bounds {
        if let syn::TypeParamBound::Trait(trait_bound) = bound {
            if let Some(segment) = trait_bound.path.segments.last() {
                if segment.ident == "Stream" {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        for arg in &args.args {
                            if let syn::GenericArgument::AssocType(assoc) = arg {
                                if assoc.ident == "Item" {
                                    return rust_type_to_sdl_type(&assoc.ty);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    "String!".to_string()
}

fn rust_type_to_sdl_type_inner(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(path) => {
            if let Some(segment) = path.path.segments.last() {
                let type_name = segment.ident.to_string();

                match type_name.as_str() {
                    "i32" | "i64" => "Int".to_string(),
                    "f32" | "f64" => "Float".to_string(),
                    "bool" => "Boolean".to_string(),
                    "String" => "String".to_string(),
                    other => other.to_string(),
                }
            } else {
                "String".to_string()
            }
        }
        _ => "String".to_string(),
    }
}
