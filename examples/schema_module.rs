use anyhow::Result;
use convoy_graphql::{Ctx, GraphQLSchema, GraphQLServer};
use futures_util::stream::{self, Stream};
use std::pin::Pin;

#[GraphQLSchema(generate = "helloworld.graphql")]
mod helloworld {
    use super::*;
    use convoy_graphql::batch;
    use std::time::Duration;

    pub struct Query;

    impl Query {
        pub async fn hello(&self, _ctx: &Ctx<'_>) -> Result<String> {
            Ok("Hello, World!".to_string())
        }

        pub async fn greet(&self, _ctx: &Ctx<'_>, name: String) -> Result<String> {
            Ok(format!("Hello, {}!", name))
        }

        pub async fn add(&self, _ctx: &Ctx<'_>, a: i64, b: i64) -> Result<i64> {
            Ok(a + b)
        }

        pub async fn users(&self, _ctx: &Ctx<'_>) -> Result<Vec<User>> {
            Ok(vec![
                User {
                    id: 1,
                    name: "Alice".to_string(),
                },
                User {
                    id: 2,
                    name: "Bob".to_string(),
                },
            ])
        }

        pub async fn user(&self, _ctx: &Ctx<'_>, id: i64) -> Result<User> {
            Ok(User {
                id,
                name: format!("User {}", id),
            })
        }
    }

    pub struct User {
        pub id: i64,
        pub name: String,
    }

    impl User {
        pub async fn id(&self, _ctx: &Ctx<'_>) -> Result<i64> {
            Ok(self.id)
        }

        pub async fn name(&self, _ctx: &Ctx<'_>) -> Result<String> {
            Ok(self.name.clone())
        }

        #[batch(key = "id", delay_ms = 1)]
        pub async fn posts(&self, _ctx: &Ctx<'_>) -> Result<Vec<Post>> {
            Ok(vec![
                Post {
                    id: self.id * 100 + 1,
                    title: format!("{}'s first post", self.name),
                },
                Post {
                    id: self.id * 100 + 2,
                    title: format!("{}'s second post", self.name),
                },
            ])
        }
    }

    pub struct Post {
        pub id: i64,
        pub title: String,
    }

    impl Post {
        pub async fn id(&self, _ctx: &Ctx<'_>) -> Result<i64> {
            Ok(self.id)
        }

        pub async fn title(&self, _ctx: &Ctx<'_>) -> Result<String> {
            Ok(self.title.clone())
        }
    }

    pub struct Subscription;

    impl Subscription {
        pub async fn counter(
            &self,
            _ctx: &Ctx<'_>,
        ) -> Pin<Box<dyn Stream<Item = Result<i64>> + Send>> {
            let stream = stream::unfold(0i64, |count| async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Some((Ok(count), count + 1))
            });
            Box::pin(stream)
        }

        pub async fn countdown(
            &self,
            _ctx: &Ctx<'_>,
        ) -> Pin<Box<dyn Stream<Item = Result<i64>> + Send>> {
            let stream = stream::unfold(10i64, |count| async move {
                if count < 0 {
                    None
                } else {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    Some((Ok(count), count - 1))
                }
            });
            Box::pin(stream)
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let schema = helloworld::Schema::build().expect("Failed to build schema");

    println!("Generated GraphQL Schema:");
    println!("{}", helloworld::SCHEMA_SDL);

    GraphQLServer::new(schema.inner().clone())
        .serve("127.0.0.1:8080")
        .await
        .map_err(|e| anyhow::anyhow!("Server error: {}", e))?;

    Ok(())
}
