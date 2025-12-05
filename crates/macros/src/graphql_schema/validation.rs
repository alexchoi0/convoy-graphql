use std::collections::HashSet;

use super::parse::ParsedModule;

pub fn validate_n_plus_one(module: &ParsedModule) -> syn::Result<()> {
    let mut list_context_types: HashSet<String> = HashSet::new();

    for s in &module.structs {
        for field in &s.fields {
            if field.is_list {
                if let Some(inner) = &field.inner_type {
                    list_context_types.insert(inner.clone());
                }
            }
        }
    }

    for impl_block in &module.impls {
        for method in &impl_block.methods {
            if method.is_list_return {
                if let Some(inner) = &method.inner_return_type {
                    list_context_types.insert(inner.clone());
                }
            }
        }
    }

    for impl_block in &module.impls {
        let type_name = impl_block.type_name.to_string();

        if type_name == "Query" || type_name == "Mutation" {
            continue;
        }

        if list_context_types.contains(&type_name) {
            for method in &impl_block.methods {
                if method.is_list_return && method.batch_config.is_none() {
                    let method_name = method.name.to_string();

                    let context_sources: Vec<String> =
                        find_list_context_sources(module, &type_name);
                    let context_hint = if context_sources.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "\n   = note: `{}` appears in list context via: {}",
                            type_name,
                            context_sources.join(", ")
                        )
                    };

                    return Err(syn::Error::new(
                        method.name.span(),
                        format!(
                            "N+1 query detected! `{}::{}` returns a list but is not batched.\n\
                             \n   = note: `{}` appears in list context, so resolvers returning lists can cause N+1{}\n\
                             \n   = help: add #[batch(key = \"field\")] to enable batching",
                            type_name, method_name, type_name, context_hint
                        ),
                    ));
                }
            }
        }
    }

    Ok(())
}

fn find_list_context_sources(module: &ParsedModule, type_name: &str) -> Vec<String> {
    let mut sources = Vec::new();

    for s in &module.structs {
        for field in &s.fields {
            if field.is_list && field.inner_type.as_deref() == Some(type_name) {
                sources.push(format!("{}.{}: Vec<{}>", s.name, field.name, type_name));
            }
        }
    }

    for impl_block in &module.impls {
        for method in &impl_block.methods {
            if method.is_list_return && method.inner_return_type.as_deref() == Some(type_name) {
                sources.push(format!(
                    "{}::{}() -> Vec<{}>",
                    impl_block.type_name, method.name, type_name
                ));
            }
        }
    }

    sources
}

#[cfg(test)]
mod tests {
    use super::*;
}
