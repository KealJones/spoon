use serde::{Deserialize, Serialize};

use crate::procedure::ProcedureId;
use crate::value::Value;

/// The neutral, decomposable procedure representation.
///
/// Not "TypeScript" or "Python" - an internal representation
/// reducible to its parts. A learned skill survives a change
/// of runtime because the skill is knowledge and the runtime
/// is an environment detail. (section 6)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Literal(Value),
    Var(String),
    BinOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    UnOp {
        op: UnOp,
        operand: Box<Expr>,
    },
    Call {
        procedure: ProcedureId,
        args: Vec<Expr>,
    },
    /// Invoke one immutable revision of an already known pure procedure.
    ///
    /// Unlike `Call`, this carries its required revision in the durable
    /// expression so a learned composition cannot silently change behavior
    /// when the dependency receives a later revision.
    CallExact {
        procedure: ProcedureId,
        version: u32,
        args: Vec<Expr>,
    },
    /// Invoke an exact procedure from the imported capability registry. This
    /// is intentionally distinct from `CallExact`: capability calls carry an
    /// effect boundary and are authorized by the host at execution time.
    CapabilityCall {
        content_id: String,
        procedure_id: String,
        input: Box<Expr>,
    },
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    Let {
        name: String,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    Block(Vec<Expr>),
    ListExpr(Vec<Expr>),
    Index {
        collection: Box<Expr>,
        index: Box<Expr>,
    },
    FieldAccess {
        object: Box<Expr>,
        field: String,
    },
    /// Iterate over a collection, applying a transform to each element.
    Map {
        collection: Box<Expr>,
        var: String,
        body: Box<Expr>,
    },
    /// Filter a collection by a predicate.
    Filter {
        collection: Box<Expr>,
        var: String,
        predicate: Box<Expr>,
    },
    /// Fold a collection into a single value.
    Reduce {
        collection: Box<Expr>,
        init: Box<Expr>,
        acc: String,
        var: String,
        body: Box<Expr>,
    },
    /// Invoke an authority-free operation from a versioned intrinsic
    /// vocabulary. Intrinsics are deterministic runtime machinery; learned
    /// procedures compose them but cannot redefine their semantics.
    Intrinsic {
        version: u16,
        op: IntrinsicOp,
        args: Vec<Expr>,
    },
}

/// Declares the version 1 operation table exactly once.
///
/// An operation needs four things to be real: an enum variant, an evaluator
/// arm, a name a lesson can use to select it, and membership in whatever list
/// advertises the vocabulary. Splitting those across hand-maintained lists is
/// how 96 operations came to be evaluable but unnameable by any lesson, so the
/// name and the variant are declared together here and everything else is
/// derived.
macro_rules! intrinsic_ops {
    (
        $(
            $(#[$meta:meta])*
            $variant:ident => $lesson_name:literal,
        )+
    ) => {
        /// Version 1 of Spoon's pure, portable standard-library operations.
        ///
        /// The derived serialization uses the variant name, and that form is
        /// written into stored procedure bodies and hashed into capability
        /// bundle content ids, so it is a persistence contract. The spelling a
        /// lesson uses is separate: see [`IntrinsicOp::lesson_name`].
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum IntrinsicOp {
            $(
                $(#[$meta])*
                $variant,
            )+
        }

        impl IntrinsicOp {
            /// Every declared operation, in declaration order.
            pub const ALL: &'static [IntrinsicOp] = &[$(IntrinsicOp::$variant,)+];

            /// The name a lesson uses to select this operation.
            pub fn lesson_name(self) -> &'static str {
                match self {
                    $(IntrinsicOp::$variant => $lesson_name,)+
                }
            }

            /// The variant identifier, which is also the stored spelling.
            pub fn variant_name(self) -> &'static str {
                match self {
                    $(IntrinsicOp::$variant => stringify!($variant),)+
                }
            }

            /// Resolves a lesson's operation name, rejecting unknown spellings.
            pub fn from_lesson_name(name: &str) -> Option<Self> {
                match name {
                    $($lesson_name => Some(IntrinsicOp::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

intrinsic_ops! {
    Length => "length",
    TextByteLength => "text_byte_length",
    TextScalarLength => "text_scalar_length",
    TextGraphemeLength => "text_grapheme_length",
    TextTokenize => "text_tokenize",
    TextSplit => "text_split",
    TextJoin => "text_join",
    TextTrim => "text_trim",
    TextLowercase => "text_lowercase",
    TextUppercase => "text_uppercase",
    TextContains => "text_contains",
    TextStartsWith => "text_starts_with",
    TextEndsWith => "text_ends_with",
    TextReplace => "text_replace",
    /// Percent-encode UTF-8 text for a URL query component. This is a pure
    /// portable transform; network authorization remains a capability call.
    TextUrlEncode => "text_url_encode",
    TextRegexCapture => "text_regex_capture",
    CollectionContains => "collection_contains",
    CollectionFindIndex => "collection_find_index",
    CountEqual => "count_equal",
    MapKeys => "map_keys",
    MapValues => "map_values",
    JsonParse => "json_parse",
    JsonStringify => "json_stringify",
    PathGet => "path_get",
    PathGetOptional => "path_get_optional",
    JsonPointerGet => "json_pointer_get",
    JsonPointerGetOptional => "json_pointer_get_optional",
    JsonPointerSet => "json_pointer_set",
    JsonPointerDelete => "json_pointer_delete",
    Coalesce => "coalesce",
    TextNormalizeNfc => "text_normalize_nfc",
    TextNormalizeNfd => "text_normalize_nfd",
    TextNormalizeNfkc => "text_normalize_nfkc",
    TextNormalizeNfkd => "text_normalize_nfkd",
    TextTrimStart => "text_trim_start",
    TextTrimEnd => "text_trim_end",
    TextGraphemeSubstring => "text_grapheme_substring",
    TextIndexOf => "text_index_of",
    TextCount => "text_count",
    TextRepeat => "text_repeat",
    TextConcatMany => "text_concat_many",
    MapEntries => "map_entries",
    MapFromEntries => "map_from_entries",
    MapSet => "map_set",
    MapDelete => "map_delete",
    MapMerge => "map_merge",
    CollectionSlice => "collection_slice",
    CollectionReverse => "collection_reverse",
    CollectionSort => "collection_sort",
    CollectionUnique => "collection_unique",
    CollectionFlatten => "collection_flatten",
    CollectionZip => "collection_zip",
    Range => "range",
    TypeName => "type_name",
    ParseInt => "parse_int",
    ParseFloat => "parse_float",
    ParseBool => "parse_bool",
    ToText => "to_text",
    NumericAbs => "numeric_abs",
    NumericSign => "numeric_sign",
    NumericMin => "numeric_min",
    NumericMax => "numeric_max",
    NumericClamp => "numeric_clamp",
    NumericFloor => "numeric_floor",
    NumericCeil => "numeric_ceil",
    NumericRound => "numeric_round",
    NumericTruncate => "numeric_truncate",
    NumericPowInt => "numeric_pow_int",
    NumericPowFloat => "numeric_pow_float",
    IntegerQuotient => "integer_quotient",
    IntegerRemainder => "integer_remainder",

    // -- Randomness (nondeterministic) --
    RandomInt => "random_int",
    RandomFloat => "random_float",
    RandomChoice => "random_choice",
    RandomShuffle => "random_shuffle",
    RandomSample => "random_sample",
    RandomUuid => "random_uuid",

    // -- Date/Time (nondeterministic: DateNow) --
    DateNow => "date_now",
    DateFromParts => "date_from_parts",
    DateGetPart => "date_get_part",
    DateAdd => "date_add",
    DateDiff => "date_diff",
    DateFormat => "date_format",

    // -- Math functions --
    MathSqrt => "math_sqrt",
    MathLog => "math_log",
    MathLog10 => "math_log10",
    MathLog2 => "math_log2",
    MathExp => "math_exp",
    MathSin => "math_sin",
    MathCos => "math_cos",
    MathTan => "math_tan",
    MathAsin => "math_asin",
    MathAcos => "math_acos",
    MathAtan => "math_atan",
    MathAtan2 => "math_atan2",
    MathPi => "math_pi",
    MathE => "math_e",
    MathIsNan => "math_is_nan",
    MathIsInfinite => "math_is_infinite",
    MathGcd => "math_gcd",
    MathLcm => "math_lcm",
    MathHypot => "math_hypot",

    // -- Text formatting --
    TextPadStart => "text_pad_start",
    TextPadEnd => "text_pad_end",
    TextSubstring => "text_substring",
    TextCharAt => "text_char_at",
    TextFormat => "text_format",
    TextMatchesRegex => "text_matches_regex",
    TextRegexReplaceAll => "text_regex_replace_all",
    TextBase64Encode => "text_base64_encode",
    TextBase64Decode => "text_base64_decode",
    TextUrlDecode => "text_url_decode",
    TextHexEncode => "text_hex_encode",
    TextHexDecode => "text_hex_decode",
    TextReverse => "text_reverse",
    TextCharCode => "text_char_code",
    TextFromCharCode => "text_from_char_code",
    TextLevenshtein => "text_levenshtein",

    // -- Hashing --
    HashSha256 => "hash_sha256",
    HashMd5 => "hash_md5",

    // -- Set operations --
    SetUnion => "set_union",
    SetIntersect => "set_intersect",
    SetDifference => "set_difference",
    SetIsSubset => "set_is_subset",

    // -- Collection extras --
    CollectionGroupBy => "collection_group_by",
    CollectionSortBy => "collection_sort_by",
    CollectionMinBy => "collection_min_by",
    CollectionMaxBy => "collection_max_by",
    CollectionChunk => "collection_chunk",
    CollectionEnumerate => "collection_enumerate",
    CollectionAny => "collection_any",
    CollectionAll => "collection_all",
    CollectionTake => "collection_take",
    CollectionDrop => "collection_drop",
    CollectionFirst => "collection_first",
    CollectionLast => "collection_last",
    CollectionPartition => "collection_partition",
    CollectionRepeatValue => "collection_repeat_value",
    CollectionWindow => "collection_window",

    // -- Map extras --
    MapHasKey => "map_has_key",
    MapGetDefault => "map_get_default",
    MapSize => "map_size",
    MapFilterKeys => "map_filter_keys",

    // -- Type checking --
    IsNull => "is_null",
    IsBool => "is_bool",
    IsInt => "is_int",
    IsFloat => "is_float",
    IsText => "is_text",
    IsList => "is_list",
    IsMap => "is_map",
    IsNumeric => "is_numeric",
    ToInt => "to_int",
    ToFloat => "to_float",
    ToBool => "to_bool",

    // -- Control --
    Assert => "assert",
    DefaultIfNull => "default_if_null",

    // -- Bitwise --
    BitAnd => "bit_and",
    BitOr => "bit_or",
    BitXor => "bit_xor",
    BitNot => "bit_not",
    BitShiftLeft => "bit_shift_left",
    BitShiftRight => "bit_shift_right",

    // -- Numeric formatting --
    NumericToFixed => "numeric_to_fixed",
    NumericToHex => "numeric_to_hex",
    NumericFromHex => "numeric_from_hex",
    NumericToBinary => "numeric_to_binary",
    NumericFromBinary => "numeric_from_binary",
}

/// An [`IntrinsicOp`] in the snake_case spelling a lesson uses.
///
/// Lesson drafts arrive as untrusted JSON naming an operation, while the
/// stored form uses the variant name. Both codecs resolve through the one
/// operation table, so a new operation cannot be admissible in storage yet
/// unnameable in a lesson.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LessonIntrinsicOp(pub IntrinsicOp);

impl Serialize for LessonIntrinsicOp {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.lesson_name())
    }
}

impl<'de> Deserialize<'de> for LessonIntrinsicOp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        IntrinsicOp::from_lesson_name(&name)
            .map(LessonIntrinsicOp)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown intrinsic operation: {name}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Not,
}

impl std::fmt::Display for BinOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BinOp::Add => write!(f, "+"),
            BinOp::Sub => write!(f, "-"),
            BinOp::Mul => write!(f, "*"),
            BinOp::Div => write!(f, "/"),
            BinOp::Mod => write!(f, "%"),
            BinOp::Eq => write!(f, "=="),
            BinOp::Ne => write!(f, "!="),
            BinOp::Lt => write!(f, "<"),
            BinOp::Le => write!(f, "<="),
            BinOp::Gt => write!(f, ">"),
            BinOp::Ge => write!(f, ">="),
            BinOp::And => write!(f, "&&"),
            BinOp::Or => write!(f, "||"),
        }
    }
}

impl std::fmt::Display for UnOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnOp::Neg => write!(f, "-"),
            UnOp::Not => write!(f, "!"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two spellings are not interchangeable and must not be unified.
    ///
    /// The stored spelling is written into procedure rows and hashed into
    /// capability bundle content ids, so changing it would invalidate every
    /// persisted procedure and every existing bundle id. The lesson spelling is
    /// what a Teacher writes. Both come from one table, and this pins each one.
    #[test]
    fn an_operation_has_a_stored_spelling_and_a_separate_lesson_spelling() {
        assert_eq!(
            serde_json::to_string(&IntrinsicOp::MathSqrt).unwrap(),
            "\"MathSqrt\""
        );
        assert_eq!(IntrinsicOp::MathSqrt.lesson_name(), "math_sqrt");

        assert_eq!(
            serde_json::to_string(&LessonIntrinsicOp(IntrinsicOp::MathSqrt)).unwrap(),
            "\"math_sqrt\""
        );
        assert_eq!(
            serde_json::from_str::<LessonIntrinsicOp>("\"math_sqrt\"").unwrap(),
            LessonIntrinsicOp(IntrinsicOp::MathSqrt)
        );
    }

    /// An operation a lesson cannot name is refused rather than defaulted.
    #[test]
    fn an_unknown_lesson_operation_name_is_refused() {
        let error = serde_json::from_str::<LessonIntrinsicOp>("\"network_fetch\"")
            .expect_err("an invented operation must not deserialize");
        assert!(
            error.to_string().contains("network_fetch"),
            "the error should name the rejected operation, got: {error}"
        );
    }

    #[test]
    fn intrinsic_expression_survives_json_round_trip() {
        let expression = Expr::Intrinsic {
            version: 1,
            op: IntrinsicOp::PathGet,
            args: vec![
                Expr::Intrinsic {
                    version: 1,
                    op: IntrinsicOp::JsonParse,
                    args: vec![Expr::Literal(Value::Text(
                        r#"{"items":[{"id":7}]}"#.to_string(),
                    ))],
                },
                Expr::Literal(Value::Text("items[0].id".to_string())),
            ],
        };

        let encoded = serde_json::to_string(&expression).unwrap();
        let decoded: Expr = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, expression);
    }
}
