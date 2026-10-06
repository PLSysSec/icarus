// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::collections::BTreeSet;

use lazy_static::lazy_static;

use cachet_lang::ast::*;
use cachet_lang::parser::*;

use crate::ast::{self as stub, Typed};

lazy_static! {
    static ref BOOL_PATH: Path = Path::from_ident("Bool");
    static ref TRUE_PATH: Path = Path::from_ident("true");
    static ref FALSE_PATH: Path = Path::from_ident("false");
    static ref INT32_PATH: Path = Path::from_ident("Int32");
    static ref INT64_PATH: Path = Path::from_ident("Int64");
    static ref CACHE_IR_PATH: Path = Path::from_ident("CacheIR");
    static ref VALUE_PATH: Path = Path::from_ident("Value");
    static ref OBJECT_PATH: Path = Path::from_ident("Object");
    static ref SHAPE_PATH: Path = Path::from_ident("Shape");
    static ref BASE_SHAPE_PATH: Path = Path::from_ident("BaseShape");
    static ref CLASS_PATH: Path = Path::from_ident("Class");
    static ref STRING_PATH: Path = Path::from_ident("String");
    static ref SYMBOL_PATH: Path = Path::from_ident("Symbol");
    static ref JSOP_PATH: Path = Path::from_ident("JSOp");
    static ref GUARD_CLASS_KIND_PATH: Path = Path::from_ident("GuardClassKind");
    static ref JS_WHY_MAGIC_PATH: Path = Path::from_ident("JSWhyMagic");
    static ref CALL_FLAGS_PATH: Path = Path::from_ident("CallFlags");
    static ref ARG_FORMAT_PATH: Path = Path::from_ident("ArgFormat");
    static ref SCALAR_TYPE_PATH: Path = Path::from_ident("ScalarType");
    static ref UNARY_MATH_FUNCTION_PATH: Path = Path::from_ident("UnaryMathFunction");
    static ref WASM_VAL_TYPE_PATH: Path = Path::from_ident("WasmValType");
    static ref JS_NATIVE_PATH: Path = Path::from_ident("JSNative");
    static ref ALLOC_KIND_PATH: Path = Path::from_ident("AllocKind");
    static ref TAGGED_PROTO_PATH: Path = Path::from_ident("TaggedProto");
}

lazy_static! {
    static ref MIR_TYPE_PATH: Path = Path::from_ident("MIRType");
    static ref UNDEFINED_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Undefined");
    static ref NULL_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Null");
    static ref BOOLEAN_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Boolean");
    static ref INT32_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Int32");
    static ref INT64_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Int64");
    static ref INT_PTR_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("IntPtr");
    static ref DOUBLE_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Double");
    static ref FLOAT32_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Float32");
    static ref STRING_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("String");
    static ref SYMBOL_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Symbol");
    static ref BIG_INT_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("BigInt");
    static ref SIMD128_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Simd128");
    static ref OBJECT_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Object");
    static ref MAGIC_OPTIMIZED_OUT_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("MagicOptimizedOut");
    static ref MAGIC_HOLE_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("MagicHole");
    static ref MAGIC_IS_CONSTRUCTING_MIR_TYPE_PATH: Path =
        MIR_TYPE_PATH.nest("MagicIsConstructing");
    static ref MAGIC_UNINITIALIZED_LEXICAL_MIR_TYPE_PATH: Path =
        MIR_TYPE_PATH.nest("MagicUninitializedLexical");
    static ref VALUE_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Value");
    static ref NONE_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("None");
    static ref SLOTS_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Slots");
    static ref ELEMENTS_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Elements");
    static ref POINTER_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Pointer");
    static ref REF_OR_NULL_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("RefOrNull");
    static ref STACK_RESULTS_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("StackResults");
    static ref SHAPE_MIR_TYPE_PATH: Path = MIR_TYPE_PATH.nest("Shape");
}

lazy_static! {
    static ref OPERAND_ID_PATH: Path = Path::from_ident("OperandId");
    static ref VALUE_ID_PATH: Path = Path::from_ident("ValueId");
    static ref OBJECT_ID_PATH: Path = Path::from_ident("ObjectId");
    static ref STRING_ID_PATH: Path = Path::from_ident("StringId");
    static ref SYMBOL_ID_PATH: Path = Path::from_ident("SymbolId");
    static ref BOOL_ID_PATH: Path = Path::from_ident("BoolId");
    static ref INT32_ID_PATH: Path = Path::from_ident("Int32Id");
    static ref NUMBER_ID_PATH: Path = Path::from_ident("NumberId");
    static ref BIG_INT_ID_PATH: Path = Path::from_ident("BigIntId");
    static ref VALUE_TAG_ID_PATH: Path = Path::from_ident("ValueTagId");
    static ref INT_PTR_ID_PATH: Path = Path::from_ident("IntPtrId");
}

lazy_static! {
    static ref INT32_FIELD_PATH: Path = Path::from_ident("Int32Field");
    static ref INT_PTR_FIELD_PATH: Path = Path::from_ident("IntPtrField");
    static ref SHAPE_FIELD_PATH: Path = Path::from_ident("ShapeField");
    static ref GETTER_SETTER_FIELD_PATH: Path = Path::from_ident("GetterSetterField");
    static ref OBJECT_FIELD_PATH: Path = Path::from_ident("ObjectField");
    static ref SYMBOL_FIELD_PATH: Path = Path::from_ident("SymbolField");
    static ref STRING_FIELD_PATH: Path = Path::from_ident("StringField");
    static ref BASE_SCRIPT_FIELD_PATH: Path = Path::from_ident("BaseScriptField");
    static ref ID_FIELD_PATH: Path = Path::from_ident("IdField");
    static ref ALLOC_SITE_FIELD_PATH: Path = Path::from_ident("AllocSiteField");
    static ref INT64_FIELD_PATH: Path = Path::from_ident("Int64Field");
    static ref VALUE_FIELD_PATH: Path = Path::from_ident("ValueField");
}

lazy_static! {
    static ref VALUE_TYPE_PATH: Path = Path::from_ident("ValueType");
    static ref DOUBLE_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("Double");
    static ref INT32_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("Int32");
    static ref BOOL_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("Bool");
    static ref UNDEFINED_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("Undefined");
    static ref NULL_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("Null");
    static ref MAGIC_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("Magic");
    static ref STRING_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("String");
    static ref SYMBOL_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("Symbol");
    static ref PRIVATE_GC_THING_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("PrivateGCThing");
    static ref BIG_INT_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("BigInt");
    static ref OBJECT_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("Object");
    static ref UNKNOWN_VALUE_TYPE_PATH: Path = VALUE_TYPE_PATH.nest("Unknown");
}

lazy_static! {
    static ref INIT_REG_STATE_PATH: Path = Path::from_ident("initRegState");
    static ref INIT_OPERAND_ID_PATH: Path = Path::from_ident("initOperandId");
    static ref INIT_VALUE_OUTPUT_PATH: Path = Path::from_ident("initValueOutput");
    static ref INIT_TYPED_OUTPUT_PATH: Path = Path::from_ident("initTypedOutput");
}

pub fn translate(stub: &stub::Stub) -> Mod {
    let params = Vec::new();

    // `Stmt::Semi`, not `Stmt::Expr`: a call in statement position carries a
    // semicolon, and only a block or an `if` may stand without one.
    let mut stmts = vec![Spanned::internal(Stmt::Semi(Expr::Invoke(Call {
        target: Spanned::internal(*INIT_REG_STATE_PATH),
        args: Spanned::internal(vec![]),
    })))];

    let operand_ids: BTreeSet<u16> = stub
        .input_operands
        .iter()
        .map(|input_operand| input_operand.data.id)
        .chain(stub.ops.iter().flat_map(|op| {
            op.args.iter().filter_map(|arg| match arg.data {
                stub::OpArgData::OperandId(operand_id) => Some(operand_id.id),
                _ => None,
            })
        }))
        .collect();
    stmts.extend(operand_ids.into_iter().map(|id| {
        Spanned::internal(Stmt::Semi(Expr::Invoke(Call {
            target: Spanned::internal(*INIT_OPERAND_ID_PATH),
            args: Spanned::internal(vec![Spanned::internal(
                Expr::Invoke(Call {
                    target: Spanned::internal(OPERAND_ID_PATH.nest("fromId")),
                    args: Spanned::internal(vec![Spanned::internal(
                        Expr::from(Literal::UInt16(id)).into(),
                    )]),
                })
                .into(),
            )]),
        })))
    }));

    stmts.extend(stub.input_operands.iter().flat_map(|input_operand| {
        [
            Spanned::internal(
                Comment {
                    text: format!("Input {input_operand}"),
                }
                .into(),
            ),
            Spanned::internal(Stmt::Semi(Expr::Invoke(Call {
                target: Spanned::internal(Path::from_ident(format!(
                    "initInput{}",
                    type_path_for_operand_id_type(input_operand.data.type_)
                ))),
                args: Spanned::internal(vec![Spanned::internal(
                    generate_operand_id_from_id_call(
                        input_operand.data.id,
                        input_operand.data.type_,
                    )
                    .into(),
                )]),
            }))),
        ]
    }));

    if let Some(output) = &stub.output {
        stmts.extend([
            Spanned::internal(
                Comment {
                    text: format!("Output {output}"),
                }
                .into(),
            ),
            Spanned::internal(Stmt::Semi(Expr::Invoke(match output {
                stub::MirType::Value => Call {
                    target: Spanned::internal(*INIT_VALUE_OUTPUT_PATH),
                    args: Spanned::internal(vec![]),
                },
                _ => Call {
                    target: Spanned::internal(*INIT_TYPED_OUTPUT_PATH),
                    args: Spanned::internal(vec![Spanned::internal(
                        Expr::from(Spanned::internal(var_path_for_mir_type(*output))).into(),
                    )]),
                },
            }))),
        ]);
    }

    for field in &stub.fields {
        let data_expr = match field.data {
            stub::FieldData::Shape(shape) => Some(generate_from_addr_call(*SHAPE_PATH, shape)),
            stub::FieldData::String(string) => Some(generate_from_addr_call(*STRING_PATH, string)),
            stub::FieldData::RawInt32(n) => Some(Literal::Int32(n).into()),
            stub::FieldData::RawInt64(n) => Some(Literal::Int64(n).into()),
            _ => None,
        };

        if let Some(data_expr) = data_expr {
            let field_expr = generate_field_from_offset_call(field.offset, field.type_());

            let read_field_expr = Expr::Invoke(Call {
                target: Spanned::internal(
                    CACHE_IR_PATH.nest(format!("read{}", type_path_for_field_type(field.type_()))),
                ),
                args: Spanned::internal(vec![Spanned::internal(field_expr.into())]),
            });

            stmts.extend([
                Spanned::internal(Stmt::from(Comment {
                    text: field.to_string(),
                })),
                Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(
                            BinOperExpr {
                                oper: Spanned::internal(CompareBinOper::Eq.into()),
                                lhs: Spanned::internal(read_field_expr),
                                rhs: Spanned::internal(data_expr),
                            }
                            .into(),
                        ),
                    }
                    .into(),
                ),
            ]);
        }
    }

    for fact in &stub.facts {
        stmts.push(Spanned::internal(
            Comment {
                text: fact.to_string(),
            }
            .into(),
        ));

        match fact {
            stub::Fact::ShapeBase(fact) => {
                stmts.push(Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(
                            BinOperExpr {
                                oper: Spanned::internal(CompareBinOper::Eq.into()),
                                lhs: Spanned::internal(Expr::Invoke(Call {
                                    target: Spanned::internal(SHAPE_PATH.nest("baseShapeOf")),
                                    args: Spanned::internal(vec![Spanned::internal(
                                        generate_from_addr_call(*SHAPE_PATH, fact.shape).into(),
                                    )]),
                                })),
                                rhs: Spanned::internal(generate_from_addr_call(
                                    *BASE_SHAPE_PATH,
                                    fact.base_shape,
                                )),
                            }
                            .into(),
                        ),
                    }
                    .into(),
                ));
            }
            stub::Fact::ShapeClass(fact) => {
                stmts.push(Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(
                            BinOperExpr {
                                oper: Spanned::internal(CompareBinOper::Eq.into()),
                                lhs: Spanned::internal(Expr::Invoke(Call {
                                    target: Spanned::internal(SHAPE_PATH.nest("classOf")),
                                    args: Spanned::internal(vec![Spanned::internal(
                                        generate_from_addr_call(*SHAPE_PATH, fact.shape).into(),
                                    )]),
                                })),
                                rhs: Spanned::internal(generate_from_addr_call(
                                    *CLASS_PATH,
                                    fact.class,
                                )),
                            }
                            .into(),
                        ),
                    }
                    .into(),
                ));
            }
            stub::Fact::ShapeNumFixedSlots(fact) => {
                stmts.push(Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(
                            BinOperExpr {
                                oper: Spanned::internal(CompareBinOper::Eq.into()),
                                lhs: Spanned::internal(Expr::Invoke(Call {
                                    target: Spanned::internal(SHAPE_PATH.nest("numFixedSlots")),
                                    args: Spanned::internal(vec![Spanned::internal(
                                        generate_from_addr_call(*SHAPE_PATH, fact.shape).into(),
                                    )]),
                                })),
                                rhs: Spanned::internal(
                                    Literal::UInt32(fact.num_fixed_slots).into(),
                                ),
                            }
                            .into(),
                        ),
                    }
                    .into(),
                ));
            }
            stub::Fact::ShapeSlotSpan(fact) => {
                stmts.push(Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(
                            BinOperExpr {
                                oper: Spanned::internal(CompareBinOper::Eq.into()),
                                lhs: Spanned::internal(Expr::Invoke(Call {
                                    target: Spanned::internal(SHAPE_PATH.nest("slotSpan")),
                                    args: Spanned::internal(vec![Spanned::internal(
                                        generate_from_addr_call(*SHAPE_PATH, fact.shape).into(),
                                    )]),
                                })),
                                rhs: Spanned::internal(Literal::UInt32(fact.slot_span).into()),
                            }
                            .into(),
                        ),
                    }
                    .into(),
                ));
            }
            stub::Fact::BaseShapeTaggedProto(fact) => {
                stmts.push(Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(
                            BinOperExpr {
                                oper: Spanned::internal(CompareBinOper::Eq.into()),
                                lhs: Spanned::internal(Expr::Invoke(Call {
                                    target: Spanned::internal(BASE_SHAPE_PATH.nest("protoOf")),
                                    args: Spanned::internal(vec![Spanned::internal(
                                        generate_from_addr_call(*BASE_SHAPE_PATH, fact.base_shape)
                                            .into(),
                                    )]),
                                })),
                                rhs: Spanned::internal(generate_from_addr_call(
                                    *TAGGED_PROTO_PATH,
                                    fact.tagged_proto,
                                )),
                            }
                            .into(),
                        ),
                    }
                    .into(),
                ));
            }
            stub::Fact::ClassIsNativeObject(fact) => {
                stmts.push(Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(Expr::Invoke(Call {
                            target: Spanned::internal(CLASS_PATH.nest("isNativeObject")),
                            args: Spanned::internal(vec![Spanned::internal(
                                generate_from_addr_call(*CLASS_PATH, fact.class).into(),
                            )]),
                        })),
                    }
                    .into(),
                ));
            }
            stub::Fact::TaggedProtoIsObject(fact) => {
                stmts.push(Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(Expr::Invoke(Call {
                            target: Spanned::internal(TAGGED_PROTO_PATH.nest("isObject")),
                            args: Spanned::internal(vec![Spanned::internal(
                                generate_from_addr_call(*TAGGED_PROTO_PATH, fact.tagged_proto)
                                    .into(),
                            )]),
                        })),
                    }
                    .into(),
                ));
            }
            stub::Fact::TaggedProtoIsLazy(fact) => {
                stmts.push(Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(Expr::Invoke(Call {
                            target: Spanned::internal(TAGGED_PROTO_PATH.nest("isLazy")),
                            args: Spanned::internal(vec![Spanned::internal(
                                generate_from_addr_call(*TAGGED_PROTO_PATH, fact.tagged_proto)
                                    .into(),
                            )]),
                        })),
                    }
                    .into(),
                ));
            }
            stub::Fact::TaggedProtoIsNull(fact) => {
                stmts.push(Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(Expr::Invoke(Call {
                            target: Spanned::internal(TAGGED_PROTO_PATH.nest("isNull")),
                            args: Spanned::internal(vec![Spanned::internal(
                                generate_from_addr_call(*TAGGED_PROTO_PATH, fact.tagged_proto)
                                    .into(),
                            )]),
                        })),
                    }
                    .into(),
                ));
            }
            stub::Fact::StringIsAtom(fact) => {
                stmts.push(Spanned::internal(
                    CheckStmt {
                        kind: CheckKind::Assume,
                        cond: Spanned::internal(Expr::Invoke(Call {
                            target: Spanned::internal(STRING_PATH.nest("isAtom")),
                            args: Spanned::internal(vec![Spanned::internal(
                                generate_from_addr_call(*STRING_PATH, fact.string).into(),
                            )]),
                        })),
                    }
                    .into(),
                ));
            }
        }
    }

    stmts.extend(stub.ops.iter().flat_map(|op| {
        [
            Spanned::internal(
                Comment {
                    text: op.to_string(),
                }
                .into(),
            ),
            Spanned::internal(Stmt::Emit(Call {
                target: Spanned::internal(CACHE_IR_PATH.nest(op.ident)),
                args: Spanned::internal(
                    op.args
                        .iter()
                        .map(|arg| {
                            Spanned::internal(
                                match &arg.data {
                                    stub::OpArgData::OperandId(data) => {
                                        generate_operand_id_from_id_call(data.id, data.type_)
                                    }
                                    stub::OpArgData::FieldOffset(data) => {
                                        generate_field_from_offset_call(data.offset, data.type_)
                                    }
                                    stub::OpArgData::ImmValue(data) => translate_imm_value(data),
                                }
                                .into(),
                            )
                        })
                        .collect(),
                ),
            })),
        ]
    }));

    let body = Block {
        stmts,
        value: Spanned::internal(None),
    };

    let op_item = CallableItem {
        ident: Spanned::internal(stub.kind),
        attrs: Vec::new(),
        is_unsafe: false,
        params,
        emits: None,
        ret: None,
        body: Spanned::internal(Some(body)),
    };

    let ir_item = IrItem {
        ident: Spanned::internal("CacheStub".into()),
        emits: Some(Spanned::internal(*CACHE_IR_PATH)),
        items: vec![Spanned::internal(Item::Op(op_item))],
    };

    Mod {
        items: vec![
            Spanned::internal(
                ImportItem {
                    file_path: Spanned::internal("../../../../../notes/cacheir.cachet".into()),
                }
                .into(),
            ),
            Spanned::internal(
                ImportItem {
                    file_path: Spanned::internal("../../../../../notes/utils.cachet".into()),
                }
                .into(),
            ),
            Spanned::internal(ir_item.into()),
        ],
    }
}

fn translate_imm_value(imm_value: &stub::ImmValue) -> Expr {
    match imm_value {
        stub::ImmValue::JSOp(ident) => Spanned::internal(JSOP_PATH.nest(*ident)).into(),
        stub::ImmValue::Bool(b) => translate_bool(*b),
        stub::ImmValue::Byte(n) => Literal::UInt8(*n).into(),
        stub::ImmValue::GuardClassKind(ident) => {
            Spanned::internal(GUARD_CLASS_KIND_PATH.nest(*ident)).into()
        }
        stub::ImmValue::ValueType(value_type) => {
            Spanned::internal(var_path_for_value_type(*value_type)).into()
        }
        stub::ImmValue::JSWhyMagic(ident) => {
            Spanned::internal(JS_WHY_MAGIC_PATH.nest(*ident)).into()
        }
        stub::ImmValue::CallFlags(call_flags) => Expr::Invoke(Call {
            target: Spanned::internal(CALL_FLAGS_PATH.nest("new")),
            args: Spanned::internal(vec![
                Spanned::internal(
                    Expr::from(Spanned::internal(
                        ARG_FORMAT_PATH.nest(call_flags.arg_format),
                    ))
                    .into(),
                ),
                Spanned::internal(translate_bool(call_flags.is_constructing).into()),
                Spanned::internal(translate_bool(call_flags.is_same_realm).into()),
                Spanned::internal(translate_bool(call_flags.needs_uninitialized_this).into()),
            ]),
        }),
        stub::ImmValue::ScalarType(ident) => {
            Spanned::internal(SCALAR_TYPE_PATH.nest(*ident)).into()
        }
        stub::ImmValue::UnaryMathFunction(ident) => {
            Spanned::internal(UNARY_MATH_FUNCTION_PATH.nest(*ident)).into()
        }
        stub::ImmValue::WasmValType(ident) => {
            Spanned::internal(WASM_VAL_TYPE_PATH.nest(*ident)).into()
        }
        stub::ImmValue::Int32(n) => Literal::Int32(*n).into(),
        stub::ImmValue::UInt32(n) => Literal::UInt32(*n).into(),
        stub::ImmValue::JSNative(addr) => generate_from_addr_call(*JS_NATIVE_PATH, *addr),
        stub::ImmValue::AllocKind(ident) => Spanned::internal(ALLOC_KIND_PATH.nest(*ident)).into(),
    }
}

fn translate_bool(b: bool) -> Expr {
    Spanned::internal(if b { *TRUE_PATH } else { *FALSE_PATH }).into()
}

fn generate_operand_id_from_id_call(id: u16, type_: stub::OperandIdType) -> Expr {
    Expr::Invoke(Call {
        target: Spanned::internal(type_path_for_operand_id_type(type_).nest("fromId")),
        args: Spanned::internal(vec![Spanned::internal(
            Expr::from(Literal::UInt16(id)).into(),
        )]),
    })
}

fn generate_field_from_offset_call(offset: u32, type_: stub::FieldType) -> Expr {
    Expr::Invoke(Call {
        target: Spanned::internal(type_path_for_field_type(type_).nest("fromOffset")),
        args: Spanned::internal(vec![Spanned::internal(
            Expr::from(Literal::UInt32(offset)).into(),
        )]),
    })
}

fn generate_from_addr_call(type_path: Path, addr: stub::Addr) -> Expr {
    Expr::Invoke(Call {
        target: Spanned::internal(type_path.nest("fromAddr")),
        args: Spanned::internal(vec![Spanned::internal(
            Expr::from(Literal::UInt64(addr)).into(),
        )]),
    })
}

fn type_path_for_operand_id_type(operand_type: stub::OperandIdType) -> Path {
    match operand_type {
        stub::OperandIdType::ValId => *VALUE_ID_PATH,
        stub::OperandIdType::ObjId => *OBJECT_ID_PATH,
        stub::OperandIdType::StringId => *STRING_ID_PATH,
        stub::OperandIdType::SymbolId => *SYMBOL_ID_PATH,
        stub::OperandIdType::BooleanId => *BOOL_ID_PATH,
        stub::OperandIdType::Int32Id => *INT32_ID_PATH,
        stub::OperandIdType::NumberId => *NUMBER_ID_PATH,
        stub::OperandIdType::BigIntId => *BIG_INT_ID_PATH,
        stub::OperandIdType::ValueTagId => *VALUE_TAG_ID_PATH,
        stub::OperandIdType::IntPtrId => *INT_PTR_ID_PATH,
        stub::OperandIdType::RawId => *OPERAND_ID_PATH,
    }
}

fn type_path_for_field_type(field_type: stub::FieldType) -> Path {
    match field_type {
        stub::FieldType::ShapeField => *SHAPE_FIELD_PATH,
        stub::FieldType::GetterSetterField => *GETTER_SETTER_FIELD_PATH,
        stub::FieldType::ObjectField => *OBJECT_FIELD_PATH,
        stub::FieldType::StringField => *STRING_FIELD_PATH,
        stub::FieldType::AtomField => type_path_for_field_type(stub::FieldType::StringField),
        stub::FieldType::PropertyNameField => {
            type_path_for_field_type(stub::FieldType::StringField)
        }
        stub::FieldType::SymbolField => *SYMBOL_FIELD_PATH,
        stub::FieldType::BaseScriptField => *BASE_SCRIPT_FIELD_PATH,
        stub::FieldType::RawInt32Field => *INT32_FIELD_PATH,
        stub::FieldType::RawPointerField => *INT_PTR_FIELD_PATH,
        stub::FieldType::IdField => *ID_FIELD_PATH,
        stub::FieldType::ValueField => *VALUE_FIELD_PATH,
        stub::FieldType::RawInt64Field => *INT64_FIELD_PATH,
        stub::FieldType::AllocSiteField => *ALLOC_SITE_FIELD_PATH,
    }
}

fn var_path_for_value_type(value_type: stub::ValueType) -> Path {
    match value_type {
        stub::ValueType::Double => *DOUBLE_VALUE_TYPE_PATH,
        stub::ValueType::Int32 => *INT32_VALUE_TYPE_PATH,
        stub::ValueType::Boolean => *BOOL_VALUE_TYPE_PATH,
        stub::ValueType::Undefined => *UNDEFINED_VALUE_TYPE_PATH,
        stub::ValueType::Null => *NULL_VALUE_TYPE_PATH,
        stub::ValueType::Magic => *MAGIC_VALUE_TYPE_PATH,
        stub::ValueType::String => *STRING_VALUE_TYPE_PATH,
        stub::ValueType::Symbol => *SYMBOL_VALUE_TYPE_PATH,
        stub::ValueType::PrivateGcThing => *PRIVATE_GC_THING_VALUE_TYPE_PATH,
        stub::ValueType::BigInt => *BIG_INT_VALUE_TYPE_PATH,
        stub::ValueType::Object => *OBJECT_VALUE_TYPE_PATH,
        stub::ValueType::Unknown => *UNKNOWN_VALUE_TYPE_PATH,
    }
}

fn var_path_for_mir_type(mir_type: stub::MirType) -> Path {
    match mir_type {
        stub::MirType::Undefined => *UNDEFINED_MIR_TYPE_PATH,
        stub::MirType::Null => *NULL_MIR_TYPE_PATH,
        stub::MirType::Bool => *BOOLEAN_MIR_TYPE_PATH,
        stub::MirType::Int32 => *INT32_MIR_TYPE_PATH,
        stub::MirType::Int64 => *INT64_MIR_TYPE_PATH,
        stub::MirType::IntPtr => *INT_PTR_MIR_TYPE_PATH,
        stub::MirType::Double => *DOUBLE_MIR_TYPE_PATH,
        stub::MirType::Float32 => *FLOAT32_MIR_TYPE_PATH,
        stub::MirType::String => *STRING_MIR_TYPE_PATH,
        stub::MirType::Symbol => *SYMBOL_MIR_TYPE_PATH,
        stub::MirType::BigInt => *BIG_INT_MIR_TYPE_PATH,
        stub::MirType::Simd128 => *SIMD128_MIR_TYPE_PATH,
        stub::MirType::Object => *OBJECT_MIR_TYPE_PATH,
        stub::MirType::MagicOptimizedOut => *MAGIC_OPTIMIZED_OUT_MIR_TYPE_PATH,
        stub::MirType::MagicHole => *MAGIC_HOLE_MIR_TYPE_PATH,
        stub::MirType::MagicIsConstructing => *MAGIC_IS_CONSTRUCTING_MIR_TYPE_PATH,
        stub::MirType::MagicUninitializedLexical => *MAGIC_UNINITIALIZED_LEXICAL_MIR_TYPE_PATH,
        stub::MirType::Value => *VALUE_MIR_TYPE_PATH,
        stub::MirType::None => *NONE_MIR_TYPE_PATH,
        stub::MirType::Slots => *SLOTS_MIR_TYPE_PATH,
        stub::MirType::Elements => *ELEMENTS_MIR_TYPE_PATH,
        stub::MirType::Pointer => *POINTER_MIR_TYPE_PATH,
        stub::MirType::RefOrNull => *REF_OR_NULL_MIR_TYPE_PATH,
        stub::MirType::StackResults => *STACK_RESULTS_MIR_TYPE_PATH,
        stub::MirType::Shape => *SHAPE_MIR_TYPE_PATH,
    }
}
