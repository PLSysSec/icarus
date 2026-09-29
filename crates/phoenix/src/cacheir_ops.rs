//! `js/src/jit/CacheIROps.yaml`: Mozilla's table of every CacheIR op.
//!
//! `CacheIROpsGenerated.h` is generated from this file by
//! `GenerateCacheIRFiles.py` (see `js/src/jit/moz.build`), so reading the same
//! file keeps our model from drifting away from the C++ we translate.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use cachet_lang::ast::{Ident, Path as CachetPath, Spanned, VarParamKind};
use cachet_lang::parser::{
    Arg, Block, Call, CallableItem, Expr, LetStmt, LocalVar, Param as CachetParam, RetStmt, Stmt,
    VarParam,
};
use indexmap::IndexMap;
use serde::Deserialize;

use crate::cpp_to_cachet::Unhandled;

/// The name of an op argument's type, e.g. `ValId`, `RawInt32Field`, `JSOpImm`.
///
/// Kept as a name rather than an enum: the set of legal types lives in
/// `arg_writer_info` in `GenerateCacheIRFiles.py`, not in the yaml, and a type
/// we don't recognize should fail where we map it, not while loading.
///
/// The suffix is the convention: `*Id` is an operand slot, `*Field` a value
/// read out of the stub's data area, `*Imm` an immediate baked into the
/// instruction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(transparent)]
pub struct ArgType(String);

impl ArgType {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArgType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One `- name: ...` entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Op {
    pub name: String,
    /// Implemented once in `CacheIRCompiler`, rather than per-compiler.
    pub shared: bool,
    /// Has a Warp transpiler case.
    pub transpile: bool,
    /// Absent on a couple of ops.
    #[serde(default)]
    pub cost_estimate: Option<u32>,
    /// The generated writer method is private and named `name_`; the public
    /// wrapper is hand-written in `CacheIRWriter.h`.
    #[serde(default)]
    pub custom_writer: bool,
    #[serde(default)]
    pub inlining_candidate: bool,
    /// Operands in the order they are written into the instruction. An arg
    /// named `result` is a slot the writer allocates rather than takes.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub args: IndexMap<String, ArgType>,
}

/// `args:` with no entries parses as null, not as an empty map.
fn null_as_empty<'de, D>(de: D) -> Result<IndexMap<String, ArgType>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::deserialize(de)?.unwrap_or_default())
}

/// The whole table, in file order, indexed by op name.
#[derive(Debug, Clone)]
pub struct Ops {
    ops: Vec<Op>,
    by_name: HashMap<String, usize>,
}

impl Ops {
    /// Where `build.rs` put mozilla-central. `None` if built with
    /// `PHOENIX_SKIP_SETUP`.
    pub fn default_path() -> Option<PathBuf> {
        option_env!("PHOENIX_MOZ_CENTRAL")
            .map(|root| Path::new(root).join("js/src/jit/CacheIROps.yaml"))
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Ops, Error> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| Error::Io(path.to_owned(), e))?;
        let ops: Vec<Op> =
            serde_yaml::from_str(&text).map_err(|e| Error::Yaml(path.to_owned(), e))?;

        let mut by_name = HashMap::with_capacity(ops.len());
        for (i, op) in ops.iter().enumerate() {
            if by_name.insert(op.name.clone(), i).is_some() {
                return Err(Error::Duplicate(op.name.clone()));
            }
        }
        Ok(Ops { ops, by_name })
    }

    pub fn get(&self, name: &str) -> Option<&Op> {
        self.by_name.get(name).map(|&i| &self.ops[i])
    }

    /// The op a `CacheIRWriter` method emits, the inverse of [`writer_method`].
    ///
    /// `guardIsNumber` and `guardIsNumber_` both name `GuardIsNumber`: the
    /// trailing `_` marks the generated method that a hand-written public
    /// wrapper calls.
    pub fn by_writer_method(&self, method: &str) -> Option<&Op> {
        let mut name = method.strip_suffix('_').unwrap_or(method).to_owned();
        if name.is_empty() {
            return None;
        }
        name[..1].make_ascii_uppercase();
        self.get(&name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Op> {
        self.ops.iter()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Yaml(PathBuf, serde_yaml::Error),
    Duplicate(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(path, e) => write!(f, "{}: {e}", path.display()),
            Error::Yaml(path, e) => write!(f, "{}: {e}", path.display()),
            Error::Duplicate(name) => write!(f, "duplicate op `{name}`"),
        }
    }
}

impl std::error::Error for Error {}

// * From the yaml to Cachet
//
// Everything below turns an entry into the Cachet that models it: the types its
// operands have, how its writer method is named, and the wrapper that makes a
// result-bearing op usable as a value. Keeping it here means one file owns what
// a CacheIR op *is*, and the C++ translator only has to ask.

/// Cachet's operand id types (notes/cacheir.cachet:143-263). Every one of them
/// is a slot number; they differ only in the static type that number carries.
pub const OPERAND_IDS: &[&str] = &[
    "ValueId",
    "ObjectId",
    "StringId",
    "SymbolId",
    "BoolId",
    "Int32Id",
    "NumberId",
    "BigIntId",
    "ValueTagId",
    "IntPtrId",
];

pub fn is_operand_id(ty: &CachetPath) -> bool {
    OPERAND_IDS.contains(&ty.to_string().as_str())
}

/// A CacheIR op argument's type to Cachet's.
///
/// Keyed on the yaml's type names rather than the C++ ones, because the yaml
/// keeps distinctions the C++ erases: `RawInt32Field` and a plain count are both
/// `uint32_t`, and `ShapeField` and `WeakShapeField` are both `Shape*`.
pub fn translate_arg_type(ty: &ArgType) -> Result<CachetPath, Unhandled> {
    let name = match ty.as_str() {
        // The operand id family, notes/cacheir.cachet:143-263.
        "ValId" => "ValueId",
        "ObjId" => "ObjectId",
        "StringId" => "StringId",
        "SymbolId" => "SymbolId",
        "BooleanId" => "BoolId",
        "Int32Id" => "Int32Id",
        "NumberId" => "NumberId",
        "BigIntId" => "BigIntId",
        "ValueTagId" => "ValueTagId",
        "IntPtrId" => "IntPtrId",
        // An operand whose type the op leaves unstated, which is what a bare
        // `OperandId` is (notes/cacheir.cachet:89).
        "RawId" => "OperandId",

        // Stub data fields, notes/cacheir.cachet:341-414. `RawInt32Field` is
        // `Int32Field` as `op LoadInt32Constant` has it (:1358).
        "RawInt32Field" => "Int32Field",
        "RawInt64Field" => "Int64Field",
        "ShapeField" => "ShapeField",
        "ObjectField" => "ObjectField",
        "StringField" => "StringField",
        "SymbolField" => "SymbolField",
        "IdField" => "IdField",
        "ValueField" => "ValueField",
        "AllocSiteField" => "AllocSiteField",

        // Immediates. `JSOpImm` is `JSOp` as `op CompareInt32Result` has it
        // (:1444), `ValueTypeImm` is `ValueType` as `op GuardNonDoubleType`
        // does (:738).
        "JSOpImm" => "JSOp",
        "ValueTypeImm" => "ValueType",
        "GuardClassKindImm" => "GuardClassKind",
        "BoolImm" => "Bool",
        "Int32Imm" => "Int32",
        "UInt32Imm" => "UInt32",
        // C++ passes a `uint32_t`, but `writeByteImm` asserts it fits in a byte
        // and writes one (CacheIRWriter.h:329).
        "ByteImm" => "UInt8",

        // Left out deliberately: the `Weak*` fields. A weak reference can be
        // cleared where its strong counterpart cannot, so the two do not denote
        // the same values and the model has no counterpart yet.
        _ => return Err(Unhandled::new(format!("CacheIR arg type `{ty}`"))),
    };
    Ok(CachetPath::from_ident(name))
}

/// A `*Field` operand: the raw value a writer method takes, and the
/// `CacheIR::write*Field` that turns it into the field the op takes.
///
/// `None` for an operand that is not a field, and for a field the model has no
/// writer for -- `writeIdField` and `writeGetterSetterField` are commented out
/// (notes/cacheir.cachet:531, :559) and the rest were never written.
pub fn field_writer(ty: &ArgType) -> Option<(&'static str, &'static str)> {
    Some(match ty.as_str() {
        // notes/cacheir.cachet:517-559.
        "RawInt32Field" => ("Int32", "writeInt32Field"),
        "ShapeField" => ("Shape", "writeShapeField"),
        "ObjectField" => ("Object", "writeObjectField"),
        "SymbolField" => ("Symbol", "writeSymbolField"),
        "StringField" => ("String", "writeStringField"),
        _ => return None,
    })
}

/// Whether an operand is stored in the stub's data area rather than encoded in
/// the instruction.
///
/// It matters because the two sides of the generated C++ see a field
/// differently: the writer method takes the value and writes it, while the
/// compiler method receives a `uint32_t` *offset* into the stub data. The model
/// takes the field on both sides, so the offset has no counterpart.
///
/// Keyed on the `Field` suffix, which is exactly the set: of the 50 arg types in
/// `arg_reader_info`, the 18 read with `reader.stubOffset()` are precisely the 18
/// whose names end in `Field`, with no exception in either direction.
pub fn is_stub_field(ty: &ArgType) -> bool {
    ty.as_str().ends_with("Field")
}

/// The `CacheIR::emitLoad*StubField` that reads a field into a register, the
/// compiler-side counterpart of [`field_writer`].
///
/// One per field kind, because the C++ generic has no counterpart: Cachet has no
/// overloading, so `emitLoadStubField`'s `switch` on `StubField::Type` had to
/// become separate functions when the model was written.
///
/// Keyed on the Cachet field type, not the yaml's, because which loader applies
/// is a fact about the model: `emitLoadInt32StubField` takes an `Int32Field`.
/// [`translate_arg_type`] already owns the yaml-to-Cachet direction and is
/// injective, so going through it loses nothing and leaves `RawInt32Field ->
/// Int32Field` recorded in one place. [`field_writer`] stays on [`ArgType`] for
/// the opposite reason: it answers what the generated *writer method* takes.
pub fn field_loader(ty: &CachetPath) -> Option<&'static str> {
    Some(match ty.to_string().as_str() {
        // notes/cacheir.cachet:2154-2166.
        "Int32Field" => "emitLoadInt32StubField",
        "ValueField" => "emitLoadValueStubField",
        "ObjectField" => "emitLoadObjectStubField",
        _ => return None,
    })
}

/// The `CacheIR::define*Id` that gives an operand id a register, keyed on the id
/// type.
///
/// One per id type, again for want of overloading: C++ has the single
/// `defineRegister`, which takes a `TypedOperandId` and reads the type back out
/// of it, while each of these converts and calls the one uninterpreted
/// `defineTypedId` (notes/cacheir.cachet:1919-1968).
///
/// A table and not a `define{ty}` rule so that it says what the model has:
/// `IntPtrId` and a bare `OperandId` are absent because there is nothing to name.
pub fn id_definer(ty: &CachetPath) -> Option<&'static str> {
    Some(match ty.to_string().as_str() {
        "ValueId" => "defineValueId",
        "ObjectId" => "defineObjectId",
        "Int32Id" => "defineInt32Id",
        "NumberId" => "defineNumberId",
        "BoolId" => "defineBoolId",
        "StringId" => "defineStringId",
        "SymbolId" => "defineSymbolId",
        "BigIntId" => "defineBigIntId",
        "ValueTagId" => "defineValueTagId",
        _ => return None,
    })
}

/// The `CacheIR::use*Id` that materializes an operand id into a register, keyed
/// on the id type. The counterpart of [`id_definer`], and the same shape.
///
/// Where defining only reserves, using reads the operand's current location and
/// may emit code to get it into a register, which is why these `emit MASM` and
/// the definers do not (notes/cacheir.cachet:1992-2098).
pub fn id_user(ty: &CachetPath) -> Option<&'static str> {
    Some(match ty.to_string().as_str() {
        "ValueId" => "useValueId",
        "ObjectId" => "useObjectId",
        "Int32Id" => "useInt32Id",
        "NumberId" => "useNumberId",
        "BoolId" => "useBoolId",
        "StringId" => "useStringId",
        "SymbolId" => "useSymbolId",
        "BigIntId" => "useBigIntId",
        "ValueTagId" => "useValueTagId",
        _ => return None,
    })
}

/// The type a *writer method* takes for an operand, which is not always the type
/// the op takes.
///
/// A `*Field` operand is a value the writer stores in the stub's data area: the
/// generated C++ method takes the raw value and writes it itself, so a wrapper
/// takes the raw value too. Every other operand passes straight through.
pub fn translate_writer_param_type(ty: &ArgType) -> Result<CachetPath, Unhandled> {
    match field_writer(ty) {
        Some((raw, _)) => Ok(CachetPath::from_ident(raw)),
        None if ty.as_str().ends_with("Field") => Err(Unhandled::new(format!(
            "CacheIR arg type `{ty}`: the model has no writer for it"
        ))),
        None => translate_arg_type(ty),
    }
}

/// The model's allocator for an operand id type.
///
/// More than a retyping: `newInt32Id` also `initOperandId`s the slot
/// (notes/cacheir.cachet:473-498), which registers where the operand lives. So
/// an id type without an allocator cannot be allocated here -- inlining
/// `fromId(takeNextOperandId())` would skip that bookkeeping -- and a wrapper
/// that would return one can't be synthesized until the model grows it.
fn id_allocator(ty: &CachetPath) -> Option<&'static str> {
    Some(match ty.to_string().as_str() {
        "ValueId" => "newValueId",
        "ObjectId" => "newObjectId",
        "Int32Id" => "newInt32Id",
        "ValueTagId" => "newValueTagId",
        _ => return None,
    })
}

/// Cachet's keywords, which an operand can't be named after. Only `op` collides
/// today; the hand-written models call that operand `jsop`.
const CACHET_KEYWORDS: &[&str] = &[
    "as", "asc", "assert", "assume", "bind", "desc", "else", "emit", "emits", "enum", "fn", "for",
    "goto", "if", "impl", "import", "in", "ir", "label", "left", "let", "mut", "op", "out",
    "return", "right", "struct", "unsafe", "var",
];

pub fn param_ident(name: &str) -> Ident {
    if CACHET_KEYWORDS.contains(&name) {
        Ident::from(format!("{name}_"))
    } else {
        Ident::from(name.to_owned())
    }
}

/// Operands are passed by value, so nothing is written back.
fn param(name: &str, type_: CachetPath) -> CachetParam {
    CachetParam::Var(VarParam {
        ident: Spanned::internal(param_ident(name)),
        kind: VarParamKind::In,
        type_: Spanned::internal(type_),
    })
}

/// The name of the `CacheIRWriter` method that emits an op, formed as
/// `GenerateCacheIRFiles.py` forms it: the op name with a lowercase first
/// letter, plus a trailing `_` when the public wrapper is hand-written.
pub fn writer_method(op: &Op) -> String {
    let mut name = op.name.clone();
    name[..1].make_ascii_lowercase();
    if op.custom_writer {
        name.push('_');
    }
    name
}

/// `CacheIR::BooleanToNumber`, the target of an `emit`.
pub fn op_path(op: &Op) -> CachetPath {
    CachetPath::from_ident("CacheIR").nest(Ident::from(op.name.clone()))
}

/// How many operands the writer method takes, which is every operand the caller
/// supplies -- so all of them but `result`.
pub fn writer_arity(op: &Op) -> usize {
    op.args.keys().filter(|name| *name != "result").count()
}

/// How a CacheIR op is declared: `op BooleanToNumber(boolean: BoolId, result: NumberId)`.
///
/// The operands are the yaml's, in the order they are written into the
/// instruction. An operand named `result` is a slot the writer allocates and
/// records rather than one the caller supplies, so it is a parameter here just
/// as it is a parameter of `CacheIRCompiler::emit<Op>`.
#[derive(Debug)]
pub struct OpSig {
    pub path: CachetPath,
    pub params: Vec<CachetParam>,
}

/// A parameter of a generated Cachet item, as a readable name-and-type pair.
///
/// Emission needs a [`CachetParam`], but translating a body needs to *read* the
/// signature, and a `CachetParam` is awkward to read back.
#[derive(Clone, Debug)]
pub struct SigParam {
    /// The yaml's name after keyword mangling, so not always the yaml's name.
    pub name: Ident,
    pub ty: CachetPath,
}

impl SigParam {
    /// The parameter as it is emitted. Operands are passed by value, so nothing
    /// is written back.
    pub fn to_param(&self) -> CachetParam {
        CachetParam::Var(VarParam {
            ident: Spanned::internal(self.name.clone()),
            kind: VarParamKind::In,
            type_: Spanned::internal(self.ty.clone()),
        })
    }
}

/// The op's parameters as [`op_sig`] binds them.
///
/// The same list `op_sig` emits, so the two cannot disagree about a name.
pub fn op_params(op: &Op) -> Result<Vec<SigParam>, Unhandled> {
    op.args
        .iter()
        .map(|(name, ty)| {
            Ok(SigParam {
                name: param_ident(name),
                ty: translate_arg_type(ty)?,
            })
        })
        .collect()
}

/// The wrapper an op needs to be usable as a value.
///
/// `emit` is a statement, so an op that allocates a result cannot appear in an
/// expression. The wrapper is the Cachet counterpart of the generated
/// `CacheIRWriter` method: allocate the slot, emit the op, return the slot.
#[derive(Debug)]
pub struct HelperSig {
    pub ident: Ident,
    pub params: Vec<CachetParam>,
    pub ret: CachetPath,
}

pub fn op_sig(op: &Op) -> Result<OpSig, Unhandled> {
    let params = op_params(op)?.iter().map(SigParam::to_param).collect();
    Ok(OpSig {
        path: op_path(op),
        params,
    })
}

/// `None` for an op that needs no wrapper, which is every op without a `result`
/// operand.
pub fn helper_sig(op: &Op) -> Result<Option<HelperSig>, Unhandled> {
    let Some(result) = op.args.get("result") else {
        return Ok(None);
    };
    let params = op
        .args
        .iter()
        .filter(|(name, _)| name.as_str() != "result")
        .map(|(name, ty)| Ok(param(name, translate_writer_param_type(ty)?)))
        .collect::<Result<Vec<_>, Unhandled>>()?;
    Ok(Some(HelperSig {
        // Named after the writer method it stands in for, so a generated call
        // traces back to the C++ it replaced.
        ident: Ident::from(writer_method(op)),
        params,
        ret: translate_arg_type(result)?,
    }))
}

/// `CacheIR::f(args)`.
fn cache_ir_call(f: &str, args: Vec<Spanned<Arg>>) -> Expr {
    Expr::Invoke(Call {
        target: Spanned::internal(
            CachetPath::from_ident("CacheIR").nest(Ident::from(f.to_owned())),
        ),
        args: Spanned::internal(args),
    })
}

fn var_expr(ident: Ident) -> Expr {
    Expr::Var(Spanned::internal(CachetPath::from_ident(ident)))
}

fn let_stmt(ident: Ident, rhs: Expr) -> Spanned<Stmt> {
    Spanned::internal(Stmt::Let(LetStmt {
        lhs: LocalVar {
            ident: Spanned::internal(ident),
            is_mut: false,
            type_: None,
        },
        rhs: Spanned::internal(rhs),
    }))
}

/// The wrapper that makes a result-bearing op usable as a value:
///
/// ```text
/// fn loadInt32Constant(val: Int32) emits CacheIR -> Int32Id {
///   let valField = CacheIR::writeInt32Field(val);
///   let result = CacheIR::newInt32Id();
///   emit CacheIR::LoadInt32Constant(valField, result);
///   return result;
/// }
/// ```
///
/// The Cachet counterpart of the generated `CacheIRWriter` method, doing what
/// `gen_writer_method` does in `GenerateCacheIRFiles.py`: write each field into
/// the stub data, allocate the result slot, emit the op, hand the slot back.
pub fn create_op_wrapper(op: &Op) -> Result<CallableItem, Unhandled> {
    let Some(helper) = helper_sig(op)? else {
        return Err(Unhandled::new(format!(
            "op `{}` allocates nothing, so it needs no wrapper",
            op.name
        )));
    };
    let allocator = id_allocator(&helper.ret).ok_or_else(|| {
        Unhandled::new(format!(
            "op `{}`: the model has no allocator for `{}`",
            op.name, helper.ret
        ))
    })?;

    let mut stmts = Vec::new();
    // The operands of the `emit`, in the order the yaml lists them, which is the
    // order they are written into the instruction.
    let mut args = Vec::new();

    for (name, ty) in &op.args {
        if name == "result" {
            let result = param_ident(name);
            stmts.push(let_stmt(result, cache_ir_call(allocator, Vec::new())));
            args.push(Spanned::internal(Arg::Expr(var_expr(result))));
            continue;
        }
        match field_writer(ty) {
            // A field is written into the stub data first, and the op takes what
            // that yields rather than the raw value.
            Some((_, writer)) => {
                let field = Ident::from(format!("{name}Field"));
                stmts.push(let_stmt(
                    field,
                    cache_ir_call(
                        writer,
                        vec![Spanned::internal(Arg::Expr(var_expr(param_ident(name))))],
                    ),
                ));
                args.push(Spanned::internal(Arg::Expr(var_expr(field))));
            }
            None => args.push(Spanned::internal(Arg::Expr(var_expr(param_ident(name))))),
        }
    }

    stmts.push(Spanned::internal(Stmt::Emit(Call {
        target: Spanned::internal(op_path(op)),
        args: Spanned::internal(args),
    })));
    stmts.push(Spanned::internal(Stmt::Ret(RetStmt {
        value: Spanned::internal(Some(var_expr(param_ident("result")))),
    })));

    Ok(CallableItem {
        ident: Spanned::internal(helper.ident),
        attrs: Vec::new(),
        is_unsafe: false,
        params: helper.params,
        emits: Some(Spanned::internal(CachetPath::from_ident("CacheIR"))),
        ret: Some(Spanned::internal(helper.ret)),
        body: Spanned::internal(Some(Block {
            stmts,
            value: Spanned::internal(None),
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(op: &Op) -> Vec<(&str, &str)> {
        op.args
            .iter()
            .map(|(name, ty)| (name.as_str(), ty.as_str()))
            .collect()
    }

    fn loaded() -> Option<Ops> {
        let path = Ops::default_path()?;
        path.exists().then(|| Ops::load(path).expect("load ops"))
    }

    fn render(params: &[CachetParam]) -> String {
        params
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The shapes a writer call can have, against the ops that have them.
    #[test]
    fn derives_signatures() {
        let Some(ops) = loaded() else { return };

        // No result: an `emit` target, no wrapper.
        let guard_is_null = ops.get("GuardIsNull").unwrap();
        let sig = op_sig(guard_is_null).unwrap();
        assert_eq!(sig.path.to_string(), "CacheIR::GuardIsNull");
        assert_eq!(render(&sig.params), "input: ValueId");
        assert_eq!(writer_method(guard_is_null), "guardIsNull");
        assert!(helper_sig(guard_is_null).unwrap().is_none());

        // A result: the op takes the slot as a parameter, and the wrapper
        // allocates it and hands it back.
        let boolean_to_number = ops.get("BooleanToNumber").unwrap();
        assert_eq!(
            render(&op_sig(boolean_to_number).unwrap().params),
            "boolean: BoolId, result: NumberId"
        );
        let helper = helper_sig(boolean_to_number).unwrap().unwrap();
        assert_eq!(helper.ident.to_string(), "booleanToNumber");
        assert_eq!(render(&helper.params), "boolean: BoolId");
        assert_eq!(helper.ret.to_string(), "NumberId");

        // A field operand has two types: the op takes the field, the writer
        // method takes the raw value it is written from.
        let load_int32_constant = ops.get("LoadInt32Constant").unwrap();
        assert_eq!(
            render(&op_sig(load_int32_constant).unwrap().params),
            "val: Int32Field, result: Int32Id"
        );
        assert_eq!(
            render(&helper_sig(load_int32_constant).unwrap().unwrap().params),
            "val: Int32"
        );

        // A custom writer: the generated method is private, hence the `_`.
        assert_eq!(
            writer_method(ops.get("GuardIsNumber").unwrap()),
            "guardIsNumber_"
        );

        // `op` is a Cachet keyword, so the operand can't keep that name.
        assert_eq!(
            render(
                &op_sig(ops.get("CompareInt32Result").unwrap())
                    .unwrap()
                    .params
            ),
            "op_: JSOp, lhs: Int32Id, rhs: Int32Id"
        );

        // Both a custom writer and a result: the wrapper the hand-written C++
        // wrapper calls.
        let load_arg = ops.get("LoadArgumentFixedSlot").unwrap();
        assert_eq!(writer_method(load_arg), "loadArgumentFixedSlot_");
        assert_eq!(
            helper_sig(load_arg).unwrap().unwrap().ret.to_string(),
            "ValueId"
        );
    }

    /// Both spellings of a custom writer's method name reach the same op.
    #[test]
    fn resolves_writer_methods() {
        let Some(ops) = loaded() else { return };

        assert_eq!(
            ops.by_writer_method("guardIsNull").unwrap().name,
            "GuardIsNull"
        );
        assert_eq!(
            ops.by_writer_method("guardIsNumber").unwrap().name,
            "GuardIsNumber"
        );
        assert_eq!(
            ops.by_writer_method("guardIsNumber_").unwrap().name,
            "GuardIsNumber"
        );
        assert!(ops.by_writer_method("notAnOp").is_none());
    }

    /// The whole wrapper, including the field write and the allocation.
    #[test]
    fn synthesizes_wrappers() {
        let Some(ops) = loaded() else { return };

        // `CallableItem` prints only through `Item`, which supplies the keyword.
        let wrapper = cachet_lang::parser::Item::Fn(
            create_op_wrapper(ops.get("LoadInt32Constant").unwrap()).unwrap(),
        );
        assert_eq!(
            wrapper.to_string().split_whitespace().collect::<Vec<_>>(),
            "fn loadInt32Constant(val: Int32) emits CacheIR -> Int32Id { \
             let valField = CacheIR::writeInt32Field(val); \
             let result = CacheIR::newInt32Id(); \
             emit CacheIR::LoadInt32Constant(valField, result); \
             return result; }"
                .split_whitespace()
                .collect::<Vec<_>>()
        );

        // `newNumberId` doesn't exist in the model, so this one can't be built.
        let e = create_op_wrapper(ops.get("BooleanToNumber").unwrap()).unwrap_err();
        assert!(e.what.contains("no allocator for `NumberId`"), "{}", e.what);
    }

    /// Counts are from the pinned revision and move when it does. The point is
    /// that every entry parses and nothing is silently dropped.
    #[test]
    fn loads_the_table() {
        let Some(path) = Ops::default_path() else {
            return; // built with PHOENIX_SKIP_SETUP
        };
        if !path.exists() {
            return; // setup hasn't run
        }
        let ops = Ops::load(&path).expect("parse CacheIROps.yaml");

        assert_eq!(ops.len(), 468);
        assert_eq!(ops.iter().filter(|o| o.custom_writer).count(), 26);
        assert_eq!(
            ops.iter()
                .filter(|o| o.args.contains_key("result"))
                .count(),
            69
        );
        assert_eq!(ops.iter().filter(|o| o.args.is_empty()).count(), 8);
        assert_eq!(ops.iter().filter(|o| o.cost_estimate.is_none()).count(), 2);

        // A plain void op.
        let guard_is_null = ops.get("GuardIsNull").unwrap();
        assert!(!guard_is_null.custom_writer);
        assert_eq!(args_of(guard_is_null), [("input", "ValId")]);

        // A result-bearing op: `result` is last, and args stay in file order.
        assert_eq!(
            args_of(ops.get("BooleanToNumber").unwrap()),
            [("boolean", "BooleanId"), ("result", "NumberId")]
        );

        // A custom writer.
        assert!(ops.get("GuardIsNumber").unwrap().custom_writer);
    }
}
