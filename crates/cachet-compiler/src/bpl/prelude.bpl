// ... begin prelude ...

type Pc = int;

type {:datatype} EmitPath;
function {:constructor} NilEmitPath(): EmitPath;
function {:constructor} ConsEmitPath(init: EmitPath, last: int): EmitPath;

type #Unit;
const #unit: #Unit;
axiom (forall x: #Unit, y: #Unit :: x == y);

// TODO(spinda): Check the validity of handling `Bool` like a numeric type in
// Boogie.
type #Bool = bool;

type #UInt16 = bv16;
type #Int16 = bv16;
type #Int32 = bv32;
type #UInt32 = bv32;
type #Int64 = bv64;
type #UInt64 = bv64;
type #Double = float53e11; // 64-bit; see https://github.com/boogie-org/boogie/issues/29#issuecomment-231239065

// Documentation on the available built-in functions can be found at:
// https://boogie-docs.readthedocs.io/en/latest/LangRef.html#other-operators

// Conversions to and from double
function {:builtin "(_ fp.to_sbv 64) RNE"}  #Double^toInt64 (n: #Double): #Int64;
function {:builtin "(_ fp.to_sbv 32) RNE"}  #Double^toInt32 (n: #Double): #Int32;
function {:builtin "(_ fp.to_sbv 16) RNE"}  #Double^toInt16 (n: #Double): #Int16;
function {:builtin "(_ fp.to_ubv 64) RNE"}  #Double^toUInt64 (n: #Double): #UInt64;
function {:builtin "(_ fp.to_ubv 32) RNE"}  #Double^toUInt32 (n: #Double): #UInt32;
function {:builtin "(_ fp.to_ubv 16) RNE"}  #Double^toUInt16 (n: #Double): #UInt16;


function {:bvbuiltin "(_ extract 15 0)"}   #Int64^to#Int16 (n: #Int64): #Int16;
function {:bvbuiltin "(_ extract 15 0)"}   #Int64^to#UInt16(n: #Int64): #UInt16;
function {:bvbuiltin "(_ extract 31 0)"}   #Int64^to#Int32 (n: #Int64): #Int32;
function {:bvbuiltin "(_ extract 31 0)"}   #Int64^to#UInt32(n: #Int64): #UInt32;
function                                   #Int64^to#UInt64(n: #Int64): #UInt64 { n }
function {:builtin "(_ to_fp 11 53) RNE"}  #Int64^to#Double(n: #Int64): #Double;

function {:bvbuiltin "(_ extract 15 0)"}   #Int32^to#Int16 (n: #Int32): #Int16;
function {:bvbuiltin "(_ extract 15 0)"}   #Int32^to#UInt16(n: #Int32): #UInt16;
function                                   #Int32^to#UInt32(n: #Int32): #UInt32 { n }
function {:bvbuiltin "(_ sign_extend 32)"} #Int32^to#Int64 (n: #Int32): #Int64;
function {:bvbuiltin "(_ sign_extend 32)"} #Int32^to#UInt64(n: #Int32): #UInt64;
function {:builtin "(_ to_fp 11 53) RNE"}  #Int32^to#Double(n: #Int32): #Double;

function                                   #Int16^to#UInt16(n: #Int16): #UInt16 { n }
function {:bvbuiltin "(_ sign_extend 16)"} #Int16^to#Int32 (n: #Int16): #Int32;
function {:bvbuiltin "(_ sign_extend 16)"} #Int16^to#UInt32(n: #Int16): #UInt32;
function {:bvbuiltin "(_ sign_extend 48)"} #Int16^to#Int64 (n: #Int16): #Int64;
function {:bvbuiltin "(_ sign_extend 48)"} #Int16^to#UInt64(n: #Int16): #UInt64;
function {:builtin "(_ to_fp 11 53) RNE"}  #Int16^to#Double(n: #Int16): #Double;

function                                   #UInt16^to#Int16 (n: #UInt16): #UInt16 { n }
function {:bvbuiltin "(_ zero_extend 16)"} #UInt16^to#Int32 (n: #UInt16): #Int32;
function {:bvbuiltin "(_ zero_extend 16)"} #UInt16^to#UInt32(n: #UInt16): #UInt32;
function {:bvbuiltin "(_ zero_extend 48)"} #UInt16^to#Int64 (n: #UInt16): #Int64;
function {:bvbuiltin "(_ zero_extend 48)"} #UInt16^to#UInt64(n: #UInt16): #UInt64;
function {:builtin "(_ to_fp 11 53) RNE"}  #UInt16^to#Double(n: #UInt16): #Double;

function {:bvbuiltin "(_ extract 15 0)"}   #UInt32^to#Int16 (n: #UInt32): #Int16;
function {:bvbuiltin "(_ extract 15 0)"}   #UInt32^to#UInt16(n: #UInt32): #UInt16;
function                                   #UInt32^to#Int32 (n: #UInt32): #Int32 { n }
function {:bvbuiltin "(_ zero_extend 32)"} #UInt32^to#Int64 (n: #UInt32): #Int64;
function {:bvbuiltin "(_ zero_extend 32)"} #UInt32^to#UInt64(n: #UInt32): #UInt64;
function {:builtin "(_ to_fp 11 53) RNE"}  #UInt32^to#Double(n: #UInt32): #Double;

function {:bvbuiltin "(_ extract 15 0)"}   #UInt64^to#Int16 (n: #UInt64): #Int16;
function {:bvbuiltin "(_ extract 15 0)"}   #UInt64^to#UInt16(n: #UInt64): #UInt16;
function {:bvbuiltin "(_ extract 31 0)"}   #UInt64^to#Int32 (n: #UInt64): #Int32;
function {:bvbuiltin "(_ extract 31 0)"}   #UInt64^to#UInt32(n: #UInt64): #UInt32;
function                                   #UInt64^to#Int64 (n: #UInt64): #Int64 { n }
function {:builtin "(_ to_fp 11 53) RNE"}  #UInt64^to#Double(n: #UInt64): #Double;

function {:bvbuiltin "bvneg"} #Int32^negate(n: #Int32): #Int32;
function {:bvbuiltin "bvadd"} #Int32^add(x: #Int32, y: #Int32): #Int32;
function {:bvbuiltin "bvsub"} #Int32^sub(x: #Int32, y: #Int32): #Int32;
function {:bvbuiltin "bvmul"} #Int32^mul(x: #Int32, y: #Int32): #Int32;
function {:bvbuiltin "bvsdiv"} #Int32^div(x: #Int32, y: #Int32): #Int32;
function {:bvbuiltin "bvsle"} #Int32^lte(a: #Int32, y: #Int32): #Bool;
function {:bvbuiltin "bvsge"} #Int32^gte(a: #Int32, y: #Int32): #Bool;
function {:bvbuiltin "bvslt"} #Int32^lt(a: #Int32, y: #Int32): #Bool;
function {:bvbuiltin "bvsgt"} #Int32^gt(a: #Int32, y: #Int32): #Bool;
function {:bvbuiltin "bvor"} #Int32^bitOr(a: #Int32, y: #Int32): #Int32;
function {:bvbuiltin "bvand"} #Int32^bitAnd(a: #Int32, y: #Int32): #Int32;
function {:bvbuiltin "bvxor"} #Int32^xor(a: #Int32, y: #Int32): #Int32;
function {:bvbuiltin "bvshl"} #Int32^shl(a: #Int32, y: #Int32): #Int32;

function {:bvbuiltin "fp.neg"} #Double^negate(n: #Double): #Double;
function {:bvbuiltin "fp.add RNE"} #Double^add(x: #Double, y: #Double): #Double;
function {:bvbuiltin "fp.sub RNE"} #Double^sub(x: #Double, y: #Double): #Double;
function {:bvbuiltin "fp.mul RNE"} #Double^mul(x: #Double, y: #Double): #Double;
function {:bvbuiltin "fp.div RNE"} #Double^div(x: #Double, y: #Double): #Double;
function {:bvbuiltin "fp.eq"} #Double^eq(x: #Double, y: #Double): #Bool;
function {:bvbuiltin "fp.leq"} #Double^lte(x: #Double, y: #Double): #Bool;
function {:bvbuiltin "fp.geq"} #Double^gte(x: #Double, y: #Double): #Bool;
function {:bvbuiltin "fp.lt"} #Double^lt(x: #Double, y: #Double): #Bool;
function {:bvbuiltin "fp.gt"} #Double^gt(x: #Double, y: #Double): #Bool;

function {:bvbuiltin "bvneg"} #Int64^negate(n: #Int64): #Int64;
function {:bvbuiltin "bvadd"} #Int64^add(x: #Int64, y: #Int64): #Int64;
function {:bvbuiltin "bvsub"} #Int64^sub(x: #Int64, y: #Int64): #Int64;
function {:bvbuiltin "bvmul"} #Int64^mul(x: #Int64, y: #Int64): #Int64;
function {:bvbuiltin "bvsdiv"} #Int64^div(x: #Int64, y: #Int64): #Int64;
function {:bvbuiltin "bvsle"} #Int64^lte(a: #Int64, y: #Int64): #Bool;
function {:bvbuiltin "bvsge"} #Int64^gte(a: #Int64, y: #Int64): #Bool;
function {:bvbuiltin "bvslt"} #Int64^lt(a: #Int64, y: #Int64): #Bool;
function {:bvbuiltin "bvsgt"} #Int64^gt(a: #Int64, y: #Int64): #Bool;
function {:bvbuiltin "bvor"} #Int64^bitOr(a: #Int64, y: #Int64): #Int64;
function {:bvbuiltin "bvand"} #Int64^bitAnd(a: #Int64, y: #Int64): #Int64;
function {:bvbuiltin "bvxor"} #Int64^xor(a: #Int64, y: #Int64): #Int64;
function {:bvbuiltin "bvshl"} #Int64^shl(a: #Int64, y: #Int64): #Int64;

function {:bvbuiltin "bvadd"} #UInt16^add(x: #UInt16, y: #UInt16): #UInt16;
function {:bvbuiltin "bvsub"} #UInt16^sub(x: #UInt16, y: #UInt16): #UInt16;
function {:bvbuiltin "bvmul"} #UInt16^mul(x: #UInt16, y: #UInt16): #UInt16;
function {:bvbuiltin "bvudiv"} #UInt16^div(x: #UInt16, y: #UInt16): #UInt16;
function {:bvbuiltin "bvsle"} #UInt16^lte(a: #UInt16, y: #UInt16): #Bool;
function {:bvbuiltin "bvsge"} #UInt16^gte(a: #UInt16, y: #UInt16): #Bool;
function {:bvbuiltin "bvslt"} #UInt16^lt(a: #UInt16, y: #UInt16): #Bool;
function {:bvbuiltin "bvsgt"} #UInt16^gt(a: #UInt16, y: #UInt16): #Bool;
function {:bvbuiltin "bvor"} #UInt16^bitOr(a: #UInt16, y: #UInt16): #UInt16;
function {:bvbuiltin "bvand"} #UInt16^bitAnd(a: #UInt16, y: #UInt16): #UInt16;
function {:bvbuiltin "bvxor"} #UInt16^xor(a: #UInt16, y: #UInt16): #UInt16;
function {:bvbuiltin "bvshl"} #UInt16^shl(a: #UInt16, y: #UInt16): #UInt16;

function {:bvbuiltin "bvadd"} #UInt64^add(x: #UInt64, y: #UInt64): #UInt64;
function {:bvbuiltin "bvsub"} #UInt64^sub(x: #UInt64, y: #UInt64): #UInt64;
function {:bvbuiltin "bvmul"} #UInt64^mul(x: #UInt64, y: #UInt64): #UInt64;
function {:bvbuiltin "bvudiv"} #UInt64^div(x: #UInt64, y: #UInt64): #UInt64;
function {:bvbuiltin "bvsle"} #UInt64^lte(a: #UInt64, y: #UInt64): #Bool;
function {:bvbuiltin "bvsge"} #UInt64^gte(a: #UInt64, y: #UInt64): #Bool;
function {:bvbuiltin "bvslt"} #UInt64^lt(a: #UInt64, y: #UInt64): #Bool;
function {:bvbuiltin "bvsgt"} #UInt64^gt(a: #UInt64, y: #UInt64): #Bool;
function {:bvbuiltin "bvor"} #UInt64^bitOr(a: #UInt64, y: #UInt64): #UInt64;
function {:bvbuiltin "bvand"} #UInt64^bitAnd(a: #UInt64, y: #UInt64): #UInt64;
function {:bvbuiltin "bvxor"} #UInt64^xor(a: #UInt64, y: #UInt64): #UInt64;
function {:bvbuiltin "bvshl"} #UInt64^shl(a: #UInt64, y: #UInt64): #UInt64;


type #Map k v = [k]v;

function {:inline} #Map~get<k, v>(map: #Map k v, key: k): v {
  map[key]
}

function {:inline} #Map~set<k, v>(map: #Map k v, key: k, value: v): #Map k v {
  map[key := value]
}

type #Set a = #Map a #Bool;

function {:inline} #Set~contains<a>(set: #Set a, value: a): #Bool {
  #Map~get(set, value)
}

function {:inline} #Set~add<a>(set: #Set a, value: a): #Set a {
  #Map~set(set, value, true)
}

function {:inline} #Set~remove<a>(set: #Set a, value: a): #Set a {
  #Map~set(set, value, false)
}

// Impls for cachet's prelude...

const #Double~INFINITY: #Double;
axiom #Double~INFINITY == 0+oo53e11;

const #Double~NEG_INFINITY: #Double;
axiom #Double~NEG_INFINITY == 0-oo53e11;

function {:builtin "fp.isNaN"} #Double~is_nan(n: #Double): #Bool;

// ... end prelude ...
