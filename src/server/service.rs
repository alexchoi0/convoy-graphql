use async_graphql::{dynamic, Request, Response, Variables};

#[derive(Clone)]
pub struct BuiltSchema {
    pub graphql_schema: dynamic::Schema,
}

impl BuiltSchema {
    pub fn from_dynamic_schema(graphql_schema: dynamic::Schema) -> Self {
        Self { graphql_schema }
    }

    pub async fn execute(&self, request: Request) -> Response {
        self.graphql_schema.execute(request).await
    }

    pub async fn execute_query(&self, query: &str) -> Response {
        let request = Request::new(query);
        self.execute(request).await
    }

    pub async fn execute_with_variables(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Response {
        let mut request = Request::new(query);
        if let Ok(vars) = serde_json::from_value::<Variables>(variables) {
            request = request.variables(vars);
        }
        self.execute(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_built_schema_from_dynamic() {
        let query = dynamic::Object::new("Query").field(dynamic::Field::new(
            "hello",
            dynamic::TypeRef::named_nn(dynamic::TypeRef::STRING),
            |_ctx| {
                dynamic::FieldFuture::new(async move {
                    Ok(Some(dynamic::FieldValue::value("world".to_string())))
                })
            },
        ));

        let schema = dynamic::Schema::build("Query", None, None)
            .register(query)
            .finish()
            .unwrap();

        let built = BuiltSchema::from_dynamic_schema(schema);
        let response = built.execute_query("{ hello }").await;

        assert!(response.errors.is_empty());
        let data = response.data.into_json().unwrap();
        assert_eq!(data["hello"], "world");
    }
}
