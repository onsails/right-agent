use crate::DbError;

enum RowInner<'row> {
    Sdk(&'row turso::Row),
    Core(&'row [turso::core::Value]),
}

pub struct Row<'row> {
    inner: RowInner<'row>,
}

impl<'row> Row<'row> {
    pub(crate) fn new(inner: &'row turso::Row) -> Self {
        Self {
            inner: RowInner::Sdk(inner),
        }
    }

    pub(crate) fn new_core(inner: &'row [turso::core::Value]) -> Self {
        Self {
            inner: RowInner::Core(inner),
        }
    }

    pub fn get<I, T>(&self, idx: I) -> Result<T, DbError>
    where
        I: TryInto<usize>,
        T: FromValue,
    {
        let idx = idx
            .try_into()
            .map_err(|_| DbError::InvalidParameter("column index does not fit in usize".into()))?;
        let value = match self.inner {
            RowInner::Sdk(row) => row.get_value(idx)?,
            RowInner::Core(row) => row
                .get(idx)
                .ok_or_else(|| DbError::InvalidParameter("column index out of range".into()))?
                .clone()
                .into(),
        };
        T::from_value(value)
    }
}

/// Internal conversion seam. Public so [`Row::get`] can bound its `T`; not
/// part of the supported `right-db` surface. Downstream code should rely on
/// the blanket impls (`i64`, `String`, `Option<T>`, etc.) via `Row::get::<T>()`
/// and must not reference [`turso::Value`] directly.
#[doc(hidden)]
pub trait FromValue: Sized {
    fn from_value(value: turso::Value) -> Result<Self, DbError>;
}

impl FromValue for turso::Value {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        Ok(value)
    }
}

impl FromValue for i64 {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        match value {
            turso::Value::Integer(value) => Ok(value),
            other => Err(invalid_type("INTEGER", &other)),
        }
    }
}

impl FromValue for i32 {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        let value = i64::from_value(value)?;
        i32::try_from(value)
            .map_err(|_| DbError::InvalidParameter("SQLite INTEGER does not fit in i32".into()))
    }
}

impl FromValue for u64 {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        let value = i64::from_value(value)?;
        u64::try_from(value)
            .map_err(|_| DbError::InvalidParameter("SQLite INTEGER is negative".into()))
    }
}

impl FromValue for u32 {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        let value = i64::from_value(value)?;
        u32::try_from(value)
            .map_err(|_| DbError::InvalidParameter("SQLite INTEGER does not fit in u32".into()))
    }
}

impl FromValue for f64 {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        match value {
            turso::Value::Real(value) => Ok(value),
            other => Err(invalid_type("REAL", &other)),
        }
    }
}

impl FromValue for bool {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        match value {
            turso::Value::Integer(0) => Ok(false),
            turso::Value::Integer(1) => Ok(true),
            turso::Value::Integer(_) => Err(DbError::InvalidParameter(
                "SQLite INTEGER is not a boolean sentinel".into(),
            )),
            other => Err(invalid_type("INTEGER", &other)),
        }
    }
}

impl FromValue for String {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        match value {
            turso::Value::Text(value) => Ok(value),
            other => Err(invalid_type("TEXT", &other)),
        }
    }
}

impl FromValue for Vec<u8> {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        match value {
            turso::Value::Blob(value) => Ok(value),
            other => Err(invalid_type("BLOB", &other)),
        }
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: turso::Value) -> Result<Self, DbError> {
        match value {
            turso::Value::Null => Ok(None),
            other => T::from_value(other).map(Some),
        }
    }
}

fn invalid_type(expected: &str, actual: &turso::Value) -> DbError {
    DbError::InvalidParameter(format!("expected SQLite {expected}, got {actual:?}"))
}
