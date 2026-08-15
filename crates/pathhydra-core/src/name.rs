use std::fmt;

macro_rules! exact_name {
    ($name:ident) => {
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Box<str>);

        impl $name {
            /// Preserves `value` exactly, including case, whitespace, and Unicode form.
            #[must_use]
            pub fn new(value: impl Into<Box<str>>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_boxed_str(self) -> Box<str> {
                self.0
            }

            #[must_use]
            pub fn into_string(self) -> String {
                self.0.into()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl From<Box<str>> for $name {
            fn from(value: Box<str>) -> Self {
                Self::new(value)
            }
        }

        impl From<$name> for Box<str> {
            fn from(value: $name) -> Self {
                value.into_boxed_str()
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.into_string()
            }
        }
    };
}

exact_name!(NodeName);
exact_name!(RelationName);

#[cfg(test)]
mod tests {
    use super::{NodeName, RelationName};

    #[test]
    fn constructors_preserve_exact_input() {
        let fixtures = [
            "token",
            "Token",
            "TOKEN",
            "token!",
            "token ",
            "tokén",
            "toke\u{301}n",
        ];

        for fixture in fixtures {
            assert_eq!(NodeName::new(fixture).as_str(), fixture);
            assert_eq!(RelationName::new(fixture).as_str(), fixture);
        }
    }
}
