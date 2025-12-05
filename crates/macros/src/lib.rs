mod graphql_schema;

use proc_macro::TokenStream;
use syn::{parse_macro_input, ItemMod};

#[proc_macro_attribute]
pub fn batch(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
#[allow(non_snake_case)]
pub fn GraphQLSchema(attr: TokenStream, item: TokenStream) -> TokenStream {
    let module = parse_macro_input!(item as ItemMod);
    let attr = proc_macro2::TokenStream::from(attr);

    match graphql_schema::expand(attr, module) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}
