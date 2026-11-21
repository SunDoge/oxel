//! Defines the CLR built-ins and their source and signature representations.

macro_rules! define_types {
    (builtins { $( $variant:ident, $alias:literal, $name:literal, $element:literal; )+ }
        other { $($other:tt)* }
    ) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Type {
            $( $variant, )+
            $($other)*
        }

        impl Type {
            pub(super) fn builtin_alias(name: &str) -> Option<Self> {
                match name {
                    $( $alias => Some(Self::$variant), )+
                    _ => None,
                }
            }

            pub(crate) fn clr_builtin(name: &str) -> Option<Self> {
                match name {
                    $( $name => Some(Self::$variant), )+
                    _ => None,
                }
            }

            pub(super) fn builtin_clr_name(self) -> Option<&'static str> {
                match self {
                    $( Self::$variant => Some(concat!("System.", $name)), )+
                    _ => None,
                }
            }

            pub(crate) fn clr_element(self) -> Option<u8> {
                match self.non_nullable() {
                    $( Self::$variant => Some($element), )+
                    _ => None,
                }
            }

            pub(crate) fn from_clr_element(element: u8) -> Option<Self> {
                match element {
                    $( $element => Some(Self::$variant), )+
                    _ => None,
                }
            }
        }
    };
}

define_types! {
    builtins {
        Void,   "void",   "Void",    0x01;
        Bool,   "bool",   "Boolean", 0x02;
        Char,   "char",   "Char",    0x03;
        I8,     "i8",     "SByte",   0x04;
        U8,     "u8",     "Byte",    0x05;
        I16,    "i16",    "Int16",   0x06;
        U16,    "u16",    "UInt16",  0x07;
        I32,    "i32",    "Int32",   0x08;
        U32,    "u32",    "UInt32",  0x09;
        I64,    "i64",    "Int64",   0x0a;
        U64,    "u64",    "UInt64",  0x0b;
        F32,    "f32",    "Single",  0x0c;
        F64,    "f64",    "Double",  0x0d;
        String, "string", "String",  0x0e;
        IntPtr, "nint",   "IntPtr",  0x18;
        UIntPtr,"nuint",  "UIntPtr", 0x19;
        Object, "object", "Object",  0x1c;
    }

    other {
        NullableString,
        NullableObject,
        NullableExternal(usize),
        Interface(usize),
        NullableInterface(usize),
        Null,
        Struct(usize),
        ClrEnum(usize),
        ClrStruct(usize),
        ConstructedClrStruct(usize),
        ConstructedStruct(usize),
        ConstructedExternal(usize),
        NullableConstructedExternal(usize),
        Array(usize),
        NullableArray(usize),
        // These IDs refer to signature templates in References, not HIR types.
        ImportedInstance(usize),
        NullableImportedInstance(usize),
        ImportedArray(usize),
        NullableImportedArray(usize),
        TypeParameter(u16),
        // Nullable parameter variants annotate imported signature templates.
        NullableTypeParameter(u16),
        External(usize),
        MethodParameter(u16),
        ImportedMethodParameter(u16),
        NullableImportedMethodParameter(u16),
        Never,
    }
}
