// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::fmt::{self, Display};

use derive_more::{Display, From};
use strum_macros::{EnumIter, IntoStaticStr};

pub use cachet_lang::ast::Ident;
use cachet_util::{fmt_join, fmt_join_leading};

pub type Word = u64;
pub type Addr = Word;

#[derive(Clone, Debug)]
pub struct Stub {
    pub kind: Ident,
    pub engine: Engine,
    pub input_operands: Vec<InputOperand>,
    pub output: Option<MirType>,
    pub ops: Vec<Op>,
    pub fields: Vec<Field>,
    pub facts: Vec<Fact>,
}

impl Display for Stub {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{} {}", self.kind, self.engine)?;
        fmt_join_leading(f, "\nInput ", self.input_operands.iter())?;
        write!(f, "\n%\n")?;
        fmt_join_leading(f, "\n", self.ops.iter())?;
        write!(f, "\n%\n")?;
        fmt_join_leading(f, "\n", self.fields.iter())?;
        write!(f, "\n%")?;
        if !self.facts.is_empty() {
            write!(f, "\n")?;
            fmt_join_leading(f, "\n", self.facts.iter())?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Display, Eq, PartialEq, Hash)]
pub enum Engine {
    Baseline,
    IonIC,
}

pub type InputOperand = Arg<OperandId>;

#[derive(Clone, Debug)]
pub struct Op {
    pub ident: Ident,
    pub args: Vec<OpArg>,
}

impl Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.ident)?;
        if !self.args.is_empty() {
            write!(f, " ")?;
            fmt_join(f, ", ", self.args.iter())?;
        }
        Ok(())
    }
}

pub trait Typed {
    type Type;

    fn type_(&self) -> Self::Type;
}

#[derive(Clone, Debug)]
pub struct Arg<T> {
    pub ident: Ident,
    pub data: T,
}

impl<T> Display for Arg<T>
where
    T: Display + Typed,
    T::Type: Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "[{} {}] {}", self.data.type_(), self.ident, self.data)
    }
}

impl<T: Typed> Typed for Arg<T> {
    type Type = T::Type;

    fn type_(&self) -> Self::Type {
        self.data.type_()
    }
}

pub type OpArg = Arg<OpArgData>;

#[derive(Clone, Copy, Debug, Display, Eq, From, Hash, PartialEq)]
pub enum OpArgType {
    OperandId(OperandIdType),
    FieldOffset(FieldType),
    ImmValue(ImmValueType),
}

#[derive(Clone, Debug, Display, From)]
pub enum OpArgData {
    OperandId(OperandId),
    FieldOffset(FieldOffset),
    ImmValue(ImmValue),
}

impl Typed for OpArgData {
    type Type = OpArgType;

    fn type_(&self) -> Self::Type {
        match self {
            Self::OperandId(data) => OpArgType::OperandId(data.type_),
            Self::FieldOffset(data) => OpArgType::FieldOffset(data.type_),
            Self::ImmValue(data) => OpArgType::ImmValue(data.type_()),
        }
    }
}

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
#[display(fmt = "{id}")]
pub struct OperandId {
    pub id: u16,
    pub type_: OperandIdType,
}

impl Typed for OperandId {
    type Type = OperandIdType;

    fn type_(&self) -> Self::Type {
        self.type_
    }
}

#[derive(Clone, Copy, Debug, Display, EnumIter, Eq, Hash, IntoStaticStr, PartialEq)]
pub enum OperandIdType {
    ValId,
    ObjId,
    StringId,
    SymbolId,
    BooleanId,
    Int32Id,
    NumberId,
    BigIntId,
    ValueTagId,
    IntPtrId,
    RawId,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "{} {offset}, {data}", "self.type_()")]
pub struct Field {
    pub offset: u32,
    pub data: FieldData,
}

impl Typed for Field {
    type Type = FieldType;

    fn type_(&self) -> Self::Type {
        self.data.type_()
    }
}

#[derive(Clone, Debug, Display)]
pub enum FieldData {
    #[display(fmt = "{_0:x}")]
    Shape(Addr),
    #[display(fmt = "{_0:x}")]
    GetterSetter(Addr),
    #[display(fmt = "{_0:x}")]
    Object(Addr),
    #[display(fmt = "{_0:x}")]
    String(Addr),
    #[display(fmt = "{_0:x}")]
    Atom(Addr),
    #[display(fmt = "{_0:x}")]
    PropertyName(Word),
    #[display(fmt = "{_0:x}")]
    Symbol(Addr),
    #[display(fmt = "{_0:x}")]
    BaseScript(Addr),
    RawInt32(i32),
    #[display(fmt = "{_0:x}")]
    RawPointer(Addr),
    #[display(fmt = "{_0:x}")]
    Id(Word),
    #[display(fmt = "{_0:x}")]
    Value(u64),
    RawInt64(i64),
    #[display(fmt = "{_0:x}")]
    AllocSite(Addr),
}

impl Typed for FieldData {
    type Type = FieldType;

    fn type_(&self) -> Self::Type {
        match self {
            Self::Shape(_) => FieldType::ShapeField,
            Self::GetterSetter(_) => FieldType::GetterSetterField,
            Self::Object(_) => FieldType::ObjectField,
            Self::String(_) => FieldType::StringField,
            Self::Atom(_) => FieldType::AtomField,
            Self::PropertyName(_) => FieldType::PropertyNameField,
            Self::Symbol(_) => FieldType::SymbolField,
            Self::BaseScript(_) => FieldType::BaseScriptField,
            Self::RawInt32(_) => FieldType::RawInt32Field,
            Self::RawPointer(_) => FieldType::RawPointerField,
            Self::Id(_) => FieldType::IdField,
            Self::Value(_) => FieldType::ValueField,
            Self::RawInt64(_) => FieldType::RawInt64Field,
            Self::AllocSite(_) => FieldType::AllocSiteField,
        }
    }
}

#[derive(Clone, Copy, Debug, Display, EnumIter, Eq, Hash, IntoStaticStr, PartialEq)]
pub enum FieldType {
    ShapeField,
    GetterSetterField,
    ObjectField,
    StringField,
    AtomField,
    PropertyNameField,
    SymbolField,
    BaseScriptField,
    RawInt32Field,
    RawPointerField,
    IdField,
    ValueField,
    RawInt64Field,
    AllocSiteField,
}

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
#[display(fmt = "{offset}")]
pub struct FieldOffset {
    pub offset: u32,
    pub type_: FieldType,
}

impl Typed for FieldOffset {
    type Type = FieldType;

    fn type_(&self) -> Self::Type {
        self.type_
    }
}

#[derive(Clone, Debug, Display, From)]
pub enum ImmValue {
    JSOp(Ident),
    Bool(bool),
    Byte(u8),
    GuardClassKind(Ident),
    ValueType(ValueType),
    JSWhyMagic(Ident),
    #[from]
    CallFlags(CallFlags),
    ScalarType(Ident),
    UnaryMathFunction(Ident),
    WasmValType(Ident),
    Int32(i32),
    UInt32(u32),
    #[display(fmt = "{_0:x}")]
    JSNative(Addr),
    // TODO(spinda): We don't have a string representation in Cachet yet.
    //StaticString(String),
    AllocKind(Ident),
}

#[derive(Clone, Copy, Debug, Display, EnumIter, Eq, Hash, IntoStaticStr, PartialEq)]
pub enum ImmValueType {
    JSOpImm,
    BoolImm,
    ByteImm,
    GuardClassKindImm,
    ValueTypeImm,
    JSWhyMagicImm,
    CallFlagsImm,
    ScalarTypeImm,
    UnaryMathFunctionImm,
    WasmValTypeImm,
    Int32Imm,
    UInt32Imm,
    JSNativeImm,
    //StaticStringImm,
    AllocKindImm,
}

impl Typed for ImmValue {
    type Type = ImmValueType;

    fn type_(&self) -> Self::Type {
        match self {
            Self::JSOp(_) => ImmValueType::JSOpImm,
            Self::Bool(_) => ImmValueType::BoolImm,
            Self::Byte(_) => ImmValueType::ByteImm,
            Self::GuardClassKind(_) => ImmValueType::GuardClassKindImm,
            Self::ValueType(_) => ImmValueType::ValueTypeImm,
            Self::JSWhyMagic(_) => ImmValueType::JSWhyMagicImm,
            Self::CallFlags(_) => ImmValueType::CallFlagsImm,
            Self::ScalarType(_) => ImmValueType::ScalarTypeImm,
            Self::UnaryMathFunction(_) => ImmValueType::UnaryMathFunctionImm,
            Self::WasmValType(_) => ImmValueType::WasmValTypeImm,
            Self::Int32(_) => ImmValueType::Int32Imm,
            Self::UInt32(_) => ImmValueType::UInt32Imm,
            Self::JSNative(_) => ImmValueType::JSNativeImm,
            //Self::StaticString(_) => ImmValueType::StaticStringImm,
            Self::AllocKind(_) => ImmValueType::AllocKindImm,
        }
    }
}

#[derive(Clone, Copy, Debug, Display, EnumIter, Eq, Hash, IntoStaticStr, PartialEq)]
pub enum ValueType {
    Double,
    Int32,
    Boolean,
    Undefined,
    Null,
    Magic,
    String,
    Symbol,
    PrivateGcThing,
    BigInt,
    Object,
    Unknown,
}

#[derive(Clone, Copy, Debug, Display, EnumIter, Eq, Hash, IntoStaticStr, PartialEq)]
pub enum MirType {
    Undefined,
    Null,
    Bool,
    Int32,
    Int64,
    IntPtr,
    Double,
    Float32,
    String,
    Symbol,
    BigInt,
    Simd128,
    Object,
    MagicOptimizedOut,
    MagicHole,
    MagicIsConstructing,
    MagicUninitializedLexical,
    Value,
    None,
    Slots,
    Elements,
    Pointer,
    RefOrNull,
    StackResults,
    Shape,
}

#[derive(Clone, Debug)]
pub struct CallFlags {
    pub arg_format: Ident,
    pub is_constructing: bool,
    pub is_same_realm: bool,
    pub needs_uninitialized_this: bool,
}

impl Display for CallFlags {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.arg_format)?;
        if self.is_constructing {
            write!(f, "+ isConstructing")?;
        }
        if self.is_same_realm {
            write!(f, "+ isSameRealm")?;
        }
        if self.needs_uninitialized_this {
            write!(f, "+ needsUninitializedThis")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Display, From)]
pub enum Fact {
    ShapeBase(ShapeBaseFact),
    ShapeClass(ShapeClassFact),
    ShapeNumFixedSlots(ShapeNumFixedSlotsFact),
    ShapeSlotSpan(ShapeSlotSpanFact),
    BaseShapeTaggedProto(BaseShapeTaggedProtoFact),
    ClassIsNativeObject(ClassIsNativeObjectFact),
    TaggedProtoIsObject(TaggedProtoIsObjectFact),
    TaggedProtoIsLazy(TaggedProtoIsLazyFact),
    TaggedProtoIsNull(TaggedProtoIsNullFact),
    StringIsAtom(StringIsAtomFact),
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "ShapeBase {shape:x}, {base_shape:x}")]
pub struct ShapeBaseFact {
    pub shape: Addr,
    pub base_shape: Addr,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "ShapeClass {shape:x}, {class:x}")]
pub struct ShapeClassFact {
    pub shape: Addr,
    pub class: Addr,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "ShapeNumFixedSlots {shape:x}, {num_fixed_slots}")]
pub struct ShapeNumFixedSlotsFact {
    pub shape: Addr,
    pub num_fixed_slots: u32,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "ShapeSlotSpan {shape:x}, {slot_span}")]
pub struct ShapeSlotSpanFact {
    pub shape: Addr,
    pub slot_span: u32,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "BaseShapeTaggedProto {base_shape:x}, {tagged_proto:x}")]
pub struct BaseShapeTaggedProtoFact {
    pub base_shape: Addr,
    pub tagged_proto: Addr,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "ClassIsNativeObject {class:x}")]
pub struct ClassIsNativeObjectFact {
    pub class: Addr,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "TaggedProtoIsObject {tagged_proto:x}")]
pub struct TaggedProtoIsObjectFact {
    pub tagged_proto: Addr,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "TaggedProtoIsLazy {tagged_proto:x}")]
pub struct TaggedProtoIsLazyFact {
    pub tagged_proto: Addr,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "TaggedProtoIsNull {tagged_proto:x}")]
pub struct TaggedProtoIsNullFact {
    pub tagged_proto: Addr,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "StringIsAtom {string:x}")]
pub struct StringIsAtomFact {
    pub string: Addr,
}
