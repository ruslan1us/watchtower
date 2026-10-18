//! A small macro for string-valued enums with an `Unknown(String)` fallback.
//!
//! Per doc/0001, unknown values from the CLI are kept as `Unknown(raw)` and
//! shown as-is, never treated as errors.

/// Defines an enum whose known variants map to fixed strings and whose
/// `Unknown(String)` variant keeps any other value. It (de)serializes as a
/// plain JSON/TOML string, implements `Display`, `From<&str>`, `From<String>`
/// and `as_str()`.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $text:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(from = "String", into = "String")]
        $vis enum $name {
            $( $(#[$vmeta])* $variant, )+
            /// A value this version of tower does not know, kept verbatim.
            Unknown(String),
        }

        impl $name {
            /// The raw string as the CLI (or the sidecar) spells it.
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $text, )+
                    Self::Unknown(raw) => raw,
                }
            }
        }

        impl From<&str> for $name {
            fn from(raw: &str) -> Self {
                match raw {
                    $( $text => Self::$variant, )+
                    other => Self::Unknown(other.to_owned()),
                }
            }
        }

        impl From<String> for $name {
            fn from(raw: String) -> Self {
                match raw.as_str() {
                    $( $text => Self::$variant, )+
                    _ => Self::Unknown(raw),
                }
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> String {
                match value {
                    $name::Unknown(raw) => raw,
                    other => other.as_str().to_owned(),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}
