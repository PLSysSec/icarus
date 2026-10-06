// vim: set tw=99 ts=4 sts=4 sw=4 et:

/*
use lalrpop_util::lalrpop_mod;
lalrpop_mod!(grammar, "/parser/grammar.rs");
use lazy_static::lazy_static;
*/

use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::character::complete::{alpha1, alphanumeric1, hex_digit1, multispace0};
//use nom::character::complete::char;
use nom::Err as ParseErr;
use nom::combinator::{complete, map, opt, recognize, value};
use nom::error::{ErrorKind as ParseErrorKind, ParseError as ParseErrorTrait};
use nom::multi::{many_till, many0, many0_count, separated_list0};
use nom::sequence::{delimited, pair, preceded, terminated};
//use nom::InputTakeAtPosition;
use strum::IntoEnumIterator;

use crate::ast::*;

pub fn parse(input: &str) -> Result<Stub, ParseError<'_>> {
    match complete(preceded(multispace0, parse_stub))(input) {
        Ok((_, stub)) => Ok(stub),
        Err(ParseErr::Error(err) | ParseErr::Failure(err)) => Err(err),
        Err(ParseErr::Incomplete(_)) => unreachable!("incomplete parse"),
    }
}

type ParseError<'a> = nom::error::VerboseError<&'a str>;
type ParseResult<'a, O> = nom::IResult<&'a str, O, ParseError<'a>>;
trait Parser<'a, O>: nom::Parser<&'a str, O, ParseError<'a>> {}
impl<'a, O, T> Parser<'a, O> for T where T: nom::Parser<&'a str, O, ParseError<'a>> {}

fn parse_stub(input: &str) -> ParseResult<'_, Stub> {
    let (input, kind) = parse_ident(input)?;
    let (input, engine) = parse_engine(input)?;
    let (input, input_operands) = many0(parse_input_operand)(input)?;
    let (input, output) = opt(parse_output)(input)?;
    let (input, _) = terminator(input)?;
    let (input, (ops, _)) = many_till(parse_op, terminator)(input)?;
    let (input, (fields, _)) = many_till(parse_field, terminator)(input)?;
    let (input, facts) = many0(parse_fact)(input)?;
    Ok((
        input,
        Stub {
            kind,
            engine,
            input_operands,
            output,
            ops,
            fields,
            facts,
        },
    ))
}

fn parse_engine(input: &str) -> ParseResult<'_, Engine> {
    alt((
        value(Engine::Baseline, symbol("Baseline")),
        value(Engine::IonIC, symbol("IonIC")),
    ))(input)
}

fn parse_input_operand(input: &str) -> ParseResult<'_, InputOperand> {
    preceded(symbol("Input"), |input| {
        parse_arg(input, parse_operand_id_type, |type_| {
            move |input| parse_operand_id(input, type_)
        })
    })(input)
}

fn parse_output(input: &str) -> ParseResult<'_, MirType> {
    preceded(symbol("Output"), parse_mir_type)(input)
}

fn parse_op(input: &str) -> ParseResult<'_, Op> {
    let (input, ident) = parse_ident(input)?;
    let (input, args) = parse_op_args(input)?;
    Ok((input, Op { ident, args }))
}

fn parse_op_args(input: &str) -> ParseResult<'_, Vec<OpArg>> {
    separated_list0(comma, parse_op_arg)(input)
}

fn parse_arg<'a, T1, T2, P1: Parser<'a, T1>, P2: Parser<'a, T2>>(
    input: &'a str,
    parse_arg_type: P1,
    parse_arg_data: impl FnOnce(T1) -> P2,
) -> ParseResult<'a, Arg<T2>> {
    let (input, (type_, ident)) =
        delimited(symbol("["), pair(parse_arg_type, parse_ident), symbol("]"))(input)?;
    let (input, data) = parse_arg_data(type_).parse(input)?;
    Ok((input, Arg { ident, data }))
}

fn parse_op_arg(input: &str) -> ParseResult<'_, OpArg> {
    parse_arg(input, parse_op_arg_type, |type_| {
        move |input| parse_op_arg_data(input, type_)
    })
}

fn parse_op_arg_type(input: &str) -> ParseResult<'_, OpArgType> {
    alt((
        map(parse_operand_id_type, Into::into),
        map(parse_field_type, Into::into),
        map(parse_imm_value_type, Into::into),
    ))(input)
}

fn parse_op_arg_data(input: &str, type_: OpArgType) -> ParseResult<'_, OpArgData> {
    match type_ {
        OpArgType::OperandId(operand_type) => {
            map(|input| parse_operand_id(input, operand_type), Into::into)(input)
        }
        OpArgType::FieldOffset(field_type) => {
            map(|input| parse_field_offset(input, field_type), Into::into)(input)
        }
        OpArgType::ImmValue(imm_type) => {
            map(|input| parse_imm_value(input, imm_type), Into::into)(input)
        }
    }
}

fn parse_operand_id(input: &str, type_: OperandIdType) -> ParseResult<'_, OperandId> {
    map(u16, |index| OperandId { id: index, type_ }.into())(input)
}

fn parse_field_offset(input: &str, type_: FieldType) -> ParseResult<'_, FieldOffset> {
    map(u32, |offset| FieldOffset { offset, type_ }.into())(input)
}

fn parse_imm_value(input: &str, type_: ImmValueType) -> ParseResult<'_, ImmValue> {
    match type_ {
        ImmValueType::JSOpImm => map(parse_ident, ImmValue::JSOp)(input),
        ImmValueType::BoolImm => map(
            alt((value(true, symbol("true")), value(false, symbol("false")))),
            ImmValue::Bool,
        )(input),
        ImmValueType::ByteImm => map(u8, ImmValue::Byte)(input),
        ImmValueType::GuardClassKindImm => map(parse_ident, ImmValue::GuardClassKind)(input),
        ImmValueType::ValueTypeImm => map(parse_value_type, ImmValue::ValueType)(input),
        ImmValueType::JSWhyMagicImm => map(parse_ident, ImmValue::JSWhyMagic)(input),
        ImmValueType::CallFlagsImm => map(parse_call_flags, ImmValue::CallFlags)(input),
        ImmValueType::ScalarTypeImm => map(parse_ident, ImmValue::ScalarType)(input),
        ImmValueType::UnaryMathFunctionImm => map(parse_ident, ImmValue::UnaryMathFunction)(input),
        ImmValueType::WasmValTypeImm => map(parse_ident, ImmValue::WasmValType)(input),
        ImmValueType::Int32Imm => map(i32, ImmValue::Int32)(input),
        ImmValueType::UInt32Imm => map(u32, ImmValue::UInt32)(input),
        ImmValueType::JSNativeImm => map(addr, ImmValue::JSNative)(input),
        /*
        ImmValueType::StaticStringImm => map(
            delimited(
                char('"'),
                |input: &str| input.split_at_position_complete(|c| c == '"'),
                char('"'),
            ),
            |s| ImmValue::StaticString(s.to_owned()),
        )(input),
        */
        ImmValueType::AllocKindImm => map(parse_ident, ImmValue::AllocKind)(input),
    }
}

fn parse_value_type(input: &str) -> ParseResult<'_, ValueType> {
    alt_enum(enum_symbol::<ValueType>)(input)
}

fn parse_mir_type(input: &str) -> ParseResult<'_, MirType> {
    alt_enum(enum_symbol::<MirType>)(input)
}

fn parse_call_flags(input: &str) -> ParseResult<'_, CallFlags> {
    let (input, arg_format) = parse_ident(input)?;

    let mut call_flags = CallFlags {
        arg_format,
        is_constructing: false,
        is_same_realm: false,
        needs_uninitialized_this: false,
    };
    let (input, _) = many0_count(preceded(
        symbol("+"),
        alt((
            map(symbol("isConstructing"), |_| {
                call_flags.is_constructing = true;
                ()
            }),
            map(symbol("isSameRealm"), |_| {
                call_flags.is_same_realm = true;
                ()
            }),
            map(symbol("needsUninitializedThis"), |_| {
                call_flags.needs_uninitialized_this = true;
                ()
            }),
        )),
    ))(input)?;

    Ok((input, call_flags))
}

fn parse_operand_id_type(input: &str) -> ParseResult<'_, OperandIdType> {
    alt_enum(enum_symbol::<OperandIdType>)(input)
}

fn parse_field_type(input: &str) -> ParseResult<'_, FieldType> {
    alt_enum(enum_symbol::<FieldType>)(input)
}

fn parse_imm_value_type(input: &str) -> ParseResult<'_, ImmValueType> {
    alt_enum(enum_symbol::<ImmValueType>)(input)
}

fn parse_field(input: &str) -> ParseResult<'_, Field> {
    let (input, type_) = parse_field_type(input)?;
    let (input, offset) = terminated(u32, comma)(input)?;
    let (input, data) = parse_field_data(input, type_)?;
    Ok((input, Field { offset, data }))
}

fn parse_field_data(input: &str, type_: FieldType) -> ParseResult<'_, FieldData> {
    match type_ {
        FieldType::ShapeField => map(addr, FieldData::Shape)(input),
        FieldType::GetterSetterField => map(addr, FieldData::GetterSetter)(input),
        FieldType::ObjectField => map(addr, FieldData::Object)(input),
        FieldType::StringField => map(addr, FieldData::String)(input),
        FieldType::AtomField => map(addr, FieldData::Atom)(input),
        FieldType::PropertyNameField => map(hex_word, FieldData::Atom)(input),
        FieldType::SymbolField => map(addr, FieldData::Symbol)(input),
        FieldType::BaseScriptField => map(addr, FieldData::BaseScript)(input),
        FieldType::RawInt32Field => map(i32, FieldData::RawInt32)(input),
        FieldType::RawPointerField => map(addr, FieldData::RawPointer)(input),
        FieldType::IdField => map(hex_word, FieldData::Id)(input),
        FieldType::ValueField => map(hex_u64, FieldData::Value)(input),
        FieldType::RawInt64Field => map(i64, FieldData::RawInt64)(input),
        FieldType::AllocSiteField => map(addr, FieldData::AllocSite)(input),
    }
}

fn parse_fact(input: &str) -> ParseResult<'_, Fact> {
    alt((
        map(parse_shape_class_fact, Into::into),
        map(parse_shape_num_fixed_slots_fact, Into::into),
        map(parse_shape_slot_span_fact, Into::into),
        map(parse_shape_base_fact, Into::into),
        map(parse_base_shape_tagged_proto_fact, Into::into),
        map(parse_class_is_native_object_fact, Into::into),
        map(parse_tagged_proto_is_object_fact, Into::into),
        map(parse_tagged_proto_is_lazy_fact, Into::into),
        map(parse_tagged_proto_is_null_fact, Into::into),
        map(parse_string_is_atom_fact, Into::into),
    ))(input)
}

fn parse_shape_base_fact(input: &str) -> ParseResult<'_, ShapeBaseFact> {
    let (input, _) = symbol("ShapeBase")(input)?;
    let (input, shape) = terminated(addr, comma)(input)?;
    let (input, base_shape) = addr(input)?;
    Ok((input, ShapeBaseFact { shape, base_shape }))
}

fn parse_shape_class_fact(input: &str) -> ParseResult<'_, ShapeClassFact> {
    let (input, _) = symbol("ShapeClass")(input)?;
    let (input, shape) = terminated(addr, comma)(input)?;
    let (input, class) = addr(input)?;
    Ok((input, ShapeClassFact { shape, class }))
}

fn parse_shape_num_fixed_slots_fact(input: &str) -> ParseResult<'_, ShapeNumFixedSlotsFact> {
    let (input, _) = symbol("ShapeNumFixedSlots")(input)?;
    let (input, shape) = terminated(addr, comma)(input)?;
    let (input, num_fixed_slots) = u32(input)?;
    Ok((
        input,
        ShapeNumFixedSlotsFact {
            shape,
            num_fixed_slots,
        },
    ))
}

fn parse_shape_slot_span_fact(input: &str) -> ParseResult<'_, ShapeSlotSpanFact> {
    let (input, _) = symbol("ShapeSlotSpan")(input)?;
    let (input, shape) = terminated(addr, comma)(input)?;
    let (input, slot_span) = u32(input)?;
    Ok((input, ShapeSlotSpanFact { shape, slot_span }))
}

fn parse_base_shape_tagged_proto_fact(input: &str) -> ParseResult<'_, BaseShapeTaggedProtoFact> {
    let (input, _) = symbol("BaseShapeTaggedProto")(input)?;
    let (input, base_shape) = terminated(addr, comma)(input)?;
    let (input, tagged_proto) = addr(input)?;
    Ok((
        input,
        BaseShapeTaggedProtoFact {
            base_shape,
            tagged_proto,
        },
    ))
}

fn parse_class_is_native_object_fact(input: &str) -> ParseResult<'_, ClassIsNativeObjectFact> {
    let (input, _) = symbol("ClassIsNativeObject")(input)?;
    let (input, class) = addr(input)?;
    Ok((input, ClassIsNativeObjectFact { class }))
}

fn parse_tagged_proto_is_object_fact(input: &str) -> ParseResult<'_, TaggedProtoIsObjectFact> {
    let (input, _) = symbol("TaggedProtoIsObject")(input)?;
    let (input, tagged_proto) = addr(input)?;
    Ok((input, TaggedProtoIsObjectFact { tagged_proto }))
}

fn parse_tagged_proto_is_lazy_fact(input: &str) -> ParseResult<'_, TaggedProtoIsLazyFact> {
    let (input, _) = symbol("TaggedProtoIsLazy")(input)?;
    let (input, tagged_proto) = addr(input)?;
    Ok((input, TaggedProtoIsLazyFact { tagged_proto }))
}

fn parse_tagged_proto_is_null_fact(input: &str) -> ParseResult<'_, TaggedProtoIsNullFact> {
    let (input, _) = symbol("TaggedProtoIsNull")(input)?;
    let (input, tagged_proto) = addr(input)?;
    Ok((input, TaggedProtoIsNullFact { tagged_proto }))
}

fn parse_string_is_atom_fact(input: &str) -> ParseResult<'_, StringIsAtomFact> {
    let (input, _) = symbol("StringIsAtom")(input)?;
    let (input, string) = addr(input)?;
    Ok((input, StringIsAtomFact { string }))
}

fn parse_ident(input: &str) -> ParseResult<'_, Ident> {
    lexeme(map(
        recognize(pair(
            alt((alpha1, tag("_"))),
            many0_count(alt((alphanumeric1, tag("_")))),
        )),
        Ident::from,
    ))(input)
}

fn u8(input: &str) -> ParseResult<'_, u8> {
    lexeme(nom::character::complete::u8)(input)
}

fn u16(input: &str) -> ParseResult<'_, u16> {
    lexeme(nom::character::complete::u16)(input)
}

fn i32(input: &str) -> ParseResult<'_, i32> {
    lexeme(nom::character::complete::i32)(input)
}

fn u32(input: &str) -> ParseResult<'_, u32> {
    lexeme(nom::character::complete::u32)(input)
}

fn i64(input: &str) -> ParseResult<'_, i64> {
    lexeme(nom::character::complete::i64)(input)
}

fn hex_u64(input: &str) -> ParseResult<'_, u64> {
    let (input, addr_str) = lexeme(hex_digit1)(input)?;
    Ok((input, u64::from_str_radix(addr_str, 16).unwrap()))
}

fn hex_word(input: &str) -> ParseResult<'_, Word> {
    hex_u64(input)
}

fn addr(input: &str) -> ParseResult<'_, Addr> {
    hex_word(input)
}

fn comma(input: &str) -> ParseResult<'_, &'_ str> {
    symbol(",")(input)
}

fn terminator(input: &str) -> ParseResult<'_, &'_ str> {
    symbol("%")(input)
}

fn lexeme<'a, O>(inner: impl Parser<'a, O>) -> impl FnMut(&'a str) -> ParseResult<'a, O> {
    terminated(inner, multispace0)
}

fn symbol<'a>(x: &'static str) -> impl FnMut(&'a str) -> ParseResult<&'a str> {
    lexeme(tag(x))
}

fn alt_enum<'a, T: IntoEnumIterator, O, P: Parser<'a, O>>(
    mut f: impl FnMut(T) -> P,
) -> impl FnMut(&'a str) -> ParseResult<'a, O> {
    move |input: &'a str| {
        let mut accum_err: Option<ParseError> = None;
        for x in T::iter() {
            match f(x).parse(input) {
                Err(ParseErr::Error(err)) => {
                    accum_err = match accum_err {
                        Some(prev_err) => Some(prev_err.or(err)),
                        None => Some(err),
                    };
                }
                res => return res,
            }
        }
        Err(ParseErr::Error(accum_err.map_or_else(
            || ParseError::from_error_kind(input, ParseErrorKind::Alt),
            |err| ParseError::append(input, ParseErrorKind::Alt, err),
        )))
    }
}

fn enum_symbol<'a, T: Copy + Into<&'static str>>(
    x: T,
) -> impl FnMut(&'a str) -> ParseResult<'a, T> {
    value(x, symbol(x.into()))
}
