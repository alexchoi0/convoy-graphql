pub mod context;
pub mod error;
pub mod loader;
pub mod server;

pub use async_graphql_value::ConstValue;
pub use context::{Ctx, FromConstValue, GraphQLType, RequestMetadata, ToConstValue};
pub use error::{Error, Result, SchemaError};
pub use loader::{BatchLoader, BoxFuture, SimpleBatchLoader};
pub use server::{BuiltSchema, GraphQLRequest, GraphQLResponse, GraphQLServer};

pub use convoy_graphql_macros::{batch, GraphQLSchema};

#[doc(hidden)]
pub mod __private {
    #[derive(Debug, Clone)]
    pub struct ResolverMeta {
        pub name: &'static str,
        pub is_batched: bool,
        pub batch_key: Option<&'static str>,
        pub batch_delay_ms: u64,
        pub is_list_return: bool,
        pub inner_return_type: Option<&'static str>,
    }

    pub trait GraphQLObjectInfo {
        const TYPE_NAME: &'static str;
        const IS_QUERY: bool;
        const IS_MUTATION: bool;

        fn list_field_types() -> &'static [&'static str];
    }

    pub trait BatchEnabled {}

    pub trait ResolverMetadata {
        fn resolver_meta() -> Vec<ResolverMeta>;

        fn all_batched() -> bool {
            Self::resolver_meta().iter().all(|r| r.is_batched)
        }

        fn unbatched_resolvers() -> Vec<&'static str> {
            Self::resolver_meta()
                .iter()
                .filter(|r| !r.is_batched)
                .map(|r| r.name)
                .collect()
        }
    }

    #[macro_export]
    macro_rules! assert_batch_enabled {
        ($ty:ty) => {
            const _: fn() = || {
                fn assert_impl<T: $crate::__private::BatchEnabled>() {}
                assert_impl::<$ty>();
            };
        };
    }
}
