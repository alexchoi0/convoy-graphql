mod autogen;
mod codegen;
mod parse;
mod validation;

use proc_macro2::TokenStream;
use syn::ItemMod;

pub fn expand(attr: TokenStream, module: ItemMod) -> syn::Result<TokenStream> {
    let args = parse::parse_macro_args(attr)?;
    let parsed = parse::parse_module(&module, args)?;
    validation::validate_n_plus_one(&parsed)?;
    codegen::generate(&parsed, &module)
}
