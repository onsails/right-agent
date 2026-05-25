use crate::DbError;

#[derive(Debug)]
#[doc(hidden)]
pub struct Params(turso::params::Params);

impl Params {
    pub(crate) fn into_turso(self) -> turso::params::Params {
        self.0
    }
}

pub trait IntoParams {
    fn into_params(self) -> Result<Params, DbError>;
}

pub trait IntoValue {
    fn into_value(self) -> Result<turso::Value, DbError>;
}

#[derive(Debug, Default)]
pub struct ParamsBuilder {
    values: Vec<turso::Value>,
    error: Option<DbError>,
}

impl ParamsBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, value: impl IntoValue) -> Result<(), DbError> {
        self.values.push(value.into_value()?);
        Ok(())
    }

    #[doc(hidden)]
    pub fn push_deferred(&mut self, value: impl IntoValue) {
        if self.error.is_some() {
            return;
        }
        match value.into_value() {
            Ok(value) => self.values.push(value),
            Err(error) => self.error = Some(error),
        }
    }
}

pub fn params_from_iter<I, T>(iter: I) -> ParamsBuilder
where
    I: IntoIterator<Item = T>,
    T: IntoValue,
{
    let mut params = ParamsBuilder::new();
    for value in iter {
        params.push_deferred(value);
    }
    params
}

#[macro_export]
macro_rules! params {
    ($($value:expr),* $(,)?) => {{
        let mut params = $crate::params::ParamsBuilder::new();
        $(
            params.push_deferred($value);
        )*
        params
    }};
}

impl IntoParams for () {
    fn into_params(self) -> Result<Params, DbError> {
        Ok(Params(turso::params::Params::None))
    }
}

impl IntoParams for [(); 0] {
    fn into_params(self) -> Result<Params, DbError> {
        Ok(Params(turso::params::Params::None))
    }
}

impl<T: IntoValue> IntoParams for Vec<T> {
    fn into_params(self) -> Result<Params, DbError> {
        self.into_iter()
            .map(IntoValue::into_value)
            .collect::<Result<Vec<_>, _>>()
            .map(|values| Params(turso::params::Params::Positional(values)))
    }
}

impl IntoParams for ParamsBuilder {
    fn into_params(self) -> Result<Params, DbError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(Params(turso::params::Params::Positional(self.values)))
    }
}

macro_rules! tuple_into_params {
    ($($name:ident),+ $(,)?) => {
        impl<$($name: IntoValue),+> IntoParams for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_params(self) -> Result<Params, DbError> {
                let ($($name,)+) = self;
                Ok(Params(turso::params::Params::Positional(vec![
                    $($name.into_value()?,)+
                ])))
            }
        }
    };
}

tuple_into_params!(A, B);
tuple_into_params!(A, B, C);
tuple_into_params!(A, B, C, D);
tuple_into_params!(A, B, C, D, E);
tuple_into_params!(A, B, C, D, E, F);
tuple_into_params!(A, B, C, D, E, F, G);
tuple_into_params!(A, B, C, D, E, F, G, H);
tuple_into_params!(A, B, C, D, E, F, G, H, I);
tuple_into_params!(A, B, C, D, E, F, G, H, I, J);
tuple_into_params!(A, B, C, D, E, F, G, H, I, J, K);
tuple_into_params!(A, B, C, D, E, F, G, H, I, J, K, L);

macro_rules! array_into_params {
    ($($len:expr),+ $(,)?) => {
        $(
            impl<T: IntoValue> IntoParams for [T; $len] {
                fn into_params(self) -> Result<Params, DbError> {
                    self.into_iter()
                        .map(IntoValue::into_value)
                        .collect::<Result<Vec<_>, _>>()
                        .map(|values| Params(turso::params::Params::Positional(values)))
                }
            }
        )+
    };
}

array_into_params!(
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
);

impl IntoValue for turso::Value {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(self)
    }
}

impl IntoValue for &str {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Text(self.to_owned()))
    }
}

impl IntoValue for String {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Text(self))
    }
}

impl IntoValue for &String {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Text(self.clone()))
    }
}

impl IntoValue for &&str {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Text((*self).to_owned()))
    }
}

impl IntoValue for i64 {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Integer(self))
    }
}

impl IntoValue for &i64 {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Integer(*self))
    }
}

impl IntoValue for i32 {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Integer(i64::from(self)))
    }
}

impl IntoValue for &i32 {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Integer(i64::from(*self)))
    }
}

impl IntoValue for u32 {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Integer(i64::from(self)))
    }
}

impl IntoValue for &u32 {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Integer(i64::from(*self)))
    }
}

impl IntoValue for u64 {
    fn into_value(self) -> Result<turso::Value, DbError> {
        let value = i64::try_from(self)
            .map_err(|_| DbError::InvalidParameter("u64 does not fit in SQLite INTEGER".into()))?;
        Ok(turso::Value::Integer(value))
    }
}

impl IntoValue for &u64 {
    fn into_value(self) -> Result<turso::Value, DbError> {
        (*self).into_value()
    }
}

impl IntoValue for bool {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Integer(i64::from(self)))
    }
}

impl IntoValue for &bool {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Integer(i64::from(*self)))
    }
}

impl IntoValue for f64 {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Real(self))
    }
}

impl IntoValue for &f64 {
    fn into_value(self) -> Result<turso::Value, DbError> {
        Ok(turso::Value::Real(*self))
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> Result<turso::Value, DbError> {
        match self {
            Some(value) => value.into_value(),
            None => Ok(turso::Value::Null),
        }
    }
}

impl<T> IntoValue for &Option<T>
where
    T: Clone + IntoValue,
{
    fn into_value(self) -> Result<turso::Value, DbError> {
        self.clone().into_value()
    }
}

#[cfg(test)]
mod tests {
    use crate::DbError;

    #[test]
    fn params_macro_defers_large_u64_error_to_execute_result_without_panic() {
        let conn = crate::Connection::open_in_memory().unwrap();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            conn.execute("SELECT ?1", crate::params![u64::MAX])
        }));

        let err = result
            .expect("large u64 parameter conversion must not panic")
            .expect_err("large u64 must be rejected as a database parameter");
        assert!(
            matches!(err, DbError::InvalidParameter(_)),
            "expected InvalidParameter, got {err:#}"
        );
    }

    #[test]
    fn params_from_iter_defers_large_u64_error_to_execute_result_without_panic() {
        let conn = crate::Connection::open_in_memory().unwrap();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            conn.execute("SELECT ?1", crate::params_from_iter([u64::MAX]))
        }));

        let err = result
            .expect("large u64 parameter conversion must not panic")
            .expect_err("large u64 must be rejected as a database parameter");
        assert!(
            matches!(err, DbError::InvalidParameter(_)),
            "expected InvalidParameter, got {err:#}"
        );
    }
}
