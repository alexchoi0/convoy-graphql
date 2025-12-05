use async_graphql::Name;
use async_graphql_value::ConstValue;
use indexmap::IndexMap;
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct RequestMetadata {
    pub headers: HashMap<String, String>,
    pub vars: HashMap<String, String>,
}

#[derive(Debug)]
pub struct Ctx<'a> {
    value: Option<&'a ConstValue>,
    args: Option<&'a IndexMap<Name, ConstValue>>,
    metadata: &'a RequestMetadata,
}

impl<'a> Ctx<'a> {
    pub fn new(
        value: Option<&'a ConstValue>,
        args: Option<&'a IndexMap<Name, ConstValue>>,
        metadata: &'a RequestMetadata,
    ) -> Self {
        Self {
            value,
            args,
            metadata,
        }
    }

    pub fn arg(&self, name: &str) -> Option<&ConstValue> {
        self.args?.get(name)
    }

    pub fn arg_as<T: FromConstValue>(&self, name: &str) -> Option<T> {
        self.arg(name).and_then(|v| T::from_const_value(v).ok())
    }

    pub fn parent(&self) -> Option<&ConstValue> {
        self.value
    }

    pub fn parent_as<T: FromConstValue>(&self) -> Option<T> {
        self.value.and_then(|v| T::from_const_value(v).ok())
    }

    pub fn parent_field(&self, name: &str) -> Option<&ConstValue> {
        match self.value? {
            ConstValue::Object(obj) => obj.get(name),
            _ => None,
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.metadata.headers.get(name).map(|s| s.as_str())
    }

    pub fn var(&self, name: &str) -> Option<&str> {
        self.metadata.vars.get(name).map(|s| s.as_str())
    }
}

pub trait ToConstValue {
    fn to_const_value(&self) -> ConstValue;
}

pub trait FromConstValue: Sized {
    fn from_const_value(value: &ConstValue) -> Result<Self, String>;
}

impl ToConstValue for i32 {
    fn to_const_value(&self) -> ConstValue {
        ConstValue::Number((*self as i64).into())
    }
}

impl FromConstValue for i32 {
    fn from_const_value(value: &ConstValue) -> Result<Self, String> {
        match value {
            ConstValue::Number(n) => n
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| "Expected i32".to_string()),
            _ => Err("Expected number".to_string()),
        }
    }
}

impl ToConstValue for i64 {
    fn to_const_value(&self) -> ConstValue {
        ConstValue::Number((*self).into())
    }
}

impl FromConstValue for i64 {
    fn from_const_value(value: &ConstValue) -> Result<Self, String> {
        match value {
            ConstValue::Number(n) => n.as_i64().ok_or_else(|| "Expected i64".to_string()),
            _ => Err("Expected number".to_string()),
        }
    }
}

impl ToConstValue for f64 {
    fn to_const_value(&self) -> ConstValue {
        ConstValue::Number(serde_json::Number::from_f64(*self).unwrap_or_else(|| 0.into()))
    }
}

impl FromConstValue for f64 {
    fn from_const_value(value: &ConstValue) -> Result<Self, String> {
        match value {
            ConstValue::Number(n) => n.as_f64().ok_or_else(|| "Expected f64".to_string()),
            _ => Err("Expected number".to_string()),
        }
    }
}

impl ToConstValue for bool {
    fn to_const_value(&self) -> ConstValue {
        ConstValue::Boolean(*self)
    }
}

impl FromConstValue for bool {
    fn from_const_value(value: &ConstValue) -> Result<Self, String> {
        match value {
            ConstValue::Boolean(b) => Ok(*b),
            _ => Err("Expected boolean".to_string()),
        }
    }
}

impl ToConstValue for String {
    fn to_const_value(&self) -> ConstValue {
        ConstValue::String(self.clone())
    }
}

impl FromConstValue for String {
    fn from_const_value(value: &ConstValue) -> Result<Self, String> {
        match value {
            ConstValue::String(s) => Ok(s.clone()),
            _ => Err("Expected string".to_string()),
        }
    }
}

impl<T: ToConstValue> ToConstValue for Option<T> {
    fn to_const_value(&self) -> ConstValue {
        match self {
            Some(v) => v.to_const_value(),
            None => ConstValue::Null,
        }
    }
}

impl<T: FromConstValue> FromConstValue for Option<T> {
    fn from_const_value(value: &ConstValue) -> Result<Self, String> {
        match value {
            ConstValue::Null => Ok(None),
            v => T::from_const_value(v).map(Some),
        }
    }
}

impl<T: ToConstValue> ToConstValue for Vec<T> {
    fn to_const_value(&self) -> ConstValue {
        ConstValue::List(self.iter().map(|v| v.to_const_value()).collect())
    }
}

impl<T: FromConstValue> FromConstValue for Vec<T> {
    fn from_const_value(value: &ConstValue) -> Result<Self, String> {
        match value {
            ConstValue::List(items) => items.iter().map(T::from_const_value).collect(),
            _ => Err("Expected list".to_string()),
        }
    }
}

pub trait GraphQLType {
    const TYPE_NAME: &'static str;
    const IS_SCALAR: bool = false;
}

impl GraphQLType for i32 {
    const TYPE_NAME: &'static str = "Int";
    const IS_SCALAR: bool = true;
}

impl GraphQLType for i64 {
    const TYPE_NAME: &'static str = "Int";
    const IS_SCALAR: bool = true;
}

impl GraphQLType for f64 {
    const TYPE_NAME: &'static str = "Float";
    const IS_SCALAR: bool = true;
}

impl GraphQLType for bool {
    const TYPE_NAME: &'static str = "Boolean";
    const IS_SCALAR: bool = true;
}

impl GraphQLType for String {
    const TYPE_NAME: &'static str = "String";
    const IS_SCALAR: bool = true;
}

impl<T: GraphQLType> GraphQLType for Option<T> {
    const TYPE_NAME: &'static str = T::TYPE_NAME;
    const IS_SCALAR: bool = T::IS_SCALAR;
}

impl<T: GraphQLType> GraphQLType for Vec<T> {
    const TYPE_NAME: &'static str = T::TYPE_NAME;
    const IS_SCALAR: bool = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_conversions() {
        assert_eq!(42i64.to_const_value(), ConstValue::Number(42.into()));
        assert_eq!(
            i64::from_const_value(&ConstValue::Number(42.into())),
            Ok(42)
        );

        assert_eq!(true.to_const_value(), ConstValue::Boolean(true));
        assert_eq!(bool::from_const_value(&ConstValue::Boolean(true)), Ok(true));

        assert_eq!(
            "hello".to_string().to_const_value(),
            ConstValue::String("hello".to_string())
        );
        assert_eq!(
            String::from_const_value(&ConstValue::String("hello".to_string())),
            Ok("hello".to_string())
        );
    }

    #[test]
    fn test_option_conversions() {
        let some_val: Option<i64> = Some(42);
        assert_eq!(some_val.to_const_value(), ConstValue::Number(42.into()));

        let none_val: Option<i64> = None;
        assert_eq!(none_val.to_const_value(), ConstValue::Null);

        assert_eq!(
            Option::<i64>::from_const_value(&ConstValue::Number(42.into())),
            Ok(Some(42))
        );
        assert_eq!(Option::<i64>::from_const_value(&ConstValue::Null), Ok(None));
    }

    #[test]
    fn test_vec_conversions() {
        let vec = vec![1i64, 2, 3];
        let expected = ConstValue::List(vec![
            ConstValue::Number(1.into()),
            ConstValue::Number(2.into()),
            ConstValue::Number(3.into()),
        ]);
        assert_eq!(vec.to_const_value(), expected);

        assert_eq!(Vec::<i64>::from_const_value(&expected), Ok(vec![1, 2, 3]));
    }
}
