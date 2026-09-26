//! One generator for every closed vocabulary in this crate.
//!
//! A closed enum here always publishes three things together: its variants, a stable
//! kebab-case `code` for each, and `ALL`. They are generated from a single list, so `ALL` and
//! `code` cannot fall behind the variants — adding a variant without a code is a compile error,
//! and adding one without extending `ALL` is impossible. A hand-maintained inventory that only
//! an adversary extends is the defect; the missing entry is only its symptom.

macro_rules! closed_enum {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident => $code:literal, $message:literal ; )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $( $(#[$variant_meta])* $variant, )+
        }

        impl $name {
            /// The stable kebab-case code for this value.
            ///
            /// Codes are part of the published contract: a reader of a refusal, a conformance
            /// observation and a document all name the same string.
            pub const fn code(self) -> &'static str {
                match self { $( Self::$variant => $code, )+ }
            }

            /// Every value this type can take.
            ///
            /// Generated from the same list as the variants, so no reader of the contract can
            /// be shown an incomplete inventory.
            pub const ALL: &'static [Self] = &[ $( Self::$variant, )+ ];
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(match self { $( Self::$variant => $message, )+ })
            }
        }
    };
}

pub(crate) use closed_enum;
