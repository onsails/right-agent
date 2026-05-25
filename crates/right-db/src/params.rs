use crate::DbError;

#[derive(Debug)]
#[doc(hidden)]
pub struct Params(libsql::params::Params);

impl Params {
    pub(crate) fn into_libsql(self) -> libsql::params::Params {
        self.0
    }
}

pub trait IntoParams {
    fn into_params(self) -> Result<Params, DbError>;
}

pub trait IntoValue {
    fn into_value(self) -> Result<libsql::Value, DbError>;
}

impl IntoParams for () {
    fn into_params(self) -> Result<Params, DbError> {
        Ok(Params(libsql::params::Params::None))
    }
}

impl IntoParams for [(); 0] {
    fn into_params(self) -> Result<Params, DbError> {
        Ok(Params(libsql::params::Params::None))
    }
}

impl<T: IntoValue> IntoParams for Vec<T> {
    fn into_params(self) -> Result<Params, DbError> {
        self.into_iter()
            .map(IntoValue::into_value)
            .collect::<Result<Vec<_>, _>>()
            .map(|values| Params(libsql::params::Params::Positional(values)))
    }
}

impl<A: IntoValue, B: IntoValue> IntoParams for (A, B) {
    fn into_params(self) -> Result<Params, DbError> {
        Ok(Params(libsql::params::Params::Positional(vec![
            self.0.into_value()?,
            self.1.into_value()?,
        ])))
    }
}

impl<A: IntoValue, B: IntoValue, C: IntoValue> IntoParams for (A, B, C) {
    fn into_params(self) -> Result<Params, DbError> {
        Ok(Params(libsql::params::Params::Positional(vec![
            self.0.into_value()?,
            self.1.into_value()?,
            self.2.into_value()?,
        ])))
    }
}

impl<A: IntoValue, B: IntoValue, C: IntoValue, D: IntoValue> IntoParams for (A, B, C, D) {
    fn into_params(self) -> Result<Params, DbError> {
        Ok(Params(libsql::params::Params::Positional(vec![
            self.0.into_value()?,
            self.1.into_value()?,
            self.2.into_value()?,
            self.3.into_value()?,
        ])))
    }
}

macro_rules! array_into_params {
    ($($len:expr),+ $(,)?) => {
        $(
            impl<T: IntoValue> IntoParams for [T; $len] {
                fn into_params(self) -> Result<Params, DbError> {
                    self.into_iter()
                        .map(IntoValue::into_value)
                        .collect::<Result<Vec<_>, _>>()
                        .map(|values| Params(libsql::params::Params::Positional(values)))
                }
            }
        )+
    };
}

array_into_params!(
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
);

impl IntoValue for libsql::Value {
    fn into_value(self) -> Result<libsql::Value, DbError> {
        Ok(self)
    }
}

impl IntoValue for &str {
    fn into_value(self) -> Result<libsql::Value, DbError> {
        Ok(libsql::Value::Text(self.to_owned()))
    }
}

impl IntoValue for String {
    fn into_value(self) -> Result<libsql::Value, DbError> {
        Ok(libsql::Value::Text(self))
    }
}

impl IntoValue for &String {
    fn into_value(self) -> Result<libsql::Value, DbError> {
        Ok(libsql::Value::Text(self.clone()))
    }
}

impl IntoValue for i64 {
    fn into_value(self) -> Result<libsql::Value, DbError> {
        Ok(libsql::Value::Integer(self))
    }
}

impl IntoValue for i32 {
    fn into_value(self) -> Result<libsql::Value, DbError> {
        Ok(libsql::Value::Integer(i64::from(self)))
    }
}

impl IntoValue for u64 {
    fn into_value(self) -> Result<libsql::Value, DbError> {
        let value = i64::try_from(self)
            .map_err(|_| DbError::InvalidParameter("u64 does not fit in SQLite INTEGER".into()))?;
        Ok(libsql::Value::Integer(value))
    }
}

impl IntoValue for bool {
    fn into_value(self) -> Result<libsql::Value, DbError> {
        Ok(libsql::Value::Integer(i64::from(self)))
    }
}

impl IntoValue for f64 {
    fn into_value(self) -> Result<libsql::Value, DbError> {
        Ok(libsql::Value::Real(self))
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> Result<libsql::Value, DbError> {
        match self {
            Some(value) => value.into_value(),
            None => Ok(libsql::Value::Null),
        }
    }
}
