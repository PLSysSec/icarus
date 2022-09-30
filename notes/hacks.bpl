type #ValueReg = #Reg;

procedure #ValueReg~scratchReg($valueReg: #ValueReg)
  returns (reg: #Reg)
{
  reg := $valueReg;
}

var #MASM~regs: #Map #Reg #RegData;

procedure #MASM~getData($reg: #Reg)
  returns (ret: #RegData)
{
  ret := #Map~get(#MASM~regs, $reg);
}

procedure #MASM~setData($reg: #Reg, $data: #RegData)
  modifies #MASM~regs;
{
  #MASM~regs := #Map~set(#MASM~regs, $reg, $data);
}
    
procedure #MASM~getValue($valueReg: #ValueReg)
  returns (ret: #Value)
{
  var tmp'0: #RegData;
  call tmp'0 := #MASM~getData($valueReg);
  call ret := #RegData~toValue(tmp'0);
}

procedure #MASM~setValue($valueReg: #ValueReg, $value: #Value)
  modifies #MASM~regs;
{
  var tmp'0: #RegData;
  call tmp'0 := #RegData~fromValue($value);
  call #MASM~setData($valueReg, tmp'0);
}

procedure #MASM~getUnboxedValue($reg: #Reg)
  returns (ret: #Value)
{
  var tmp'0: #RegData;
  call tmp'0 := #MASM~getData($reg);
  call ret := #RegData~toUnboxedValue(tmp'0);
}

procedure #MASM~setUnboxedValue($reg: #Reg, $value: #Value)
  modifies #MASM~regs;
{
  var tmp'0: #RegData;
  call tmp'0 := #RegData~fromUnboxedValue($value);
  call #MASM~setData($reg, tmp'0);
}

procedure #MASM~getInt32($reg: #Reg)
  returns (ret: #Int32)
{
  var tmp'0: #Value;
  call tmp'0 := #MASM~getUnboxedValue($reg);
  call ret := #Value~toInt32(tmp'0);
}

procedure #MASM~setInt32($reg: #Reg, $int32: #Int32)
  modifies #MASM~regs;
{
  var tmp'0: #Value;
  call tmp'0 := #Value~fromInt32($int32);
  call #MASM~setUnboxedValue($reg, tmp'0);
}

procedure #MASM~getBool($reg: #Reg)
  returns (ret: #Bool)
{
  var tmp'0: #Value;
  call tmp'0 := #MASM~getUnboxedValue($reg);
  call ret := #Value~toBool(tmp'0);
}

procedure #MASM~setBool($reg: #Reg, $bool: #Bool)
  modifies #MASM~regs;
{
  var tmp'0: #Value;
  call tmp'0 := #Value~fromBool($bool);
  call #MASM~setUnboxedValue($reg, tmp'0);
}

procedure #MASM~getObject($reg: #Reg)
  returns (ret: #Object)
{
  var tmp'0: #Value;
  call tmp'0 := #MASM~getUnboxedValue($reg);
  call ret := #Value~toObject(tmp'0);
}

procedure #MASM~setObject($reg: #Reg, $object: #Object)
  modifies #MASM~regs;
{
  var tmp'0: #Value;
  call tmp'0 := #Value~fromObject($object);
  call #MASM~setUnboxedValue($reg, tmp'0);
}

procedure #MASM~getString($reg: #Reg)
  returns (ret: #String)
{
  var tmp'0: #Value;
  call tmp'0 := #MASM~getUnboxedValue($reg);
  call ret := #Value~toString(tmp'0);
}

procedure #MASM~setString($reg: #Reg, $string: #String)
  modifies #MASM~regs;
{
  var tmp'0: #Value;
  call tmp'0 := #Value~fromString($string);
  call #MASM~setUnboxedValue($reg, tmp'0);
}

procedure #MASM~getSymbol($reg: #Reg)
  returns (ret: #Symbol)
{
  var tmp'0: #Value;
  call tmp'0 := #MASM~getUnboxedValue($reg);
  call ret := #Value~toSymbol(tmp'0);
}

procedure #MASM~setSymbol($reg: #Reg, $symbol: #Symbol)
  modifies #MASM~regs;
{
  var tmp'0: #Value;
  call tmp'0 := #Value~fromSymbol($symbol);
  call #MASM~setUnboxedValue($reg, tmp'0);
}

procedure #MASM~getBigInt($reg: #Reg)
  returns (ret: #BigInt)
{
  var tmp'0: #Value;
  call tmp'0 := #MASM~getUnboxedValue($reg);
  call ret := #Value~toBigInt(tmp'0);
}

procedure #MASM~setBigInt($reg: #Reg, $bigInt: #BigInt)
  modifies #MASM~regs;
{
  var tmp'0: #Value;
  call tmp'0 := #Value~fromBigInt($bigInt);
  call #MASM~setUnboxedValue($reg, tmp'0);
}

var #MASM~floatRegs: #Map #FloatReg #Double;

procedure #MASM~getDouble($floatReg: #FloatReg)
  returns (ret: #Double)
{
  ret := #Map~get(#MASM~floatRegs, $floatReg);
}

procedure #MASM~setDouble($floatReg: #FloatReg, $double: #Double)
  modifies #MASM~regs;
{
    #MASM~floatRegs := #Map~set(#MASM~floatRegs, $floatReg, $double);
}

var #MASM~stackPtr: #UInt64;
var #MASM~stack: #Map #UInt64 #RegData;

procedure #MASM~stackPush($data: #RegData)
  modifies #MASM~stackPtr, #MASM~stack;
{
    #MASM~stack := #Map~set(#MASM~stack, #MASM~stackPtr, $data);
    #MASM~stackPtr := #MASM~stackPtr + 8;
}

procedure #MASM~stackPop()
  returns (data: #RegData)
  modifies #MASM~stackPtr, #MASM~stack;
{
    data := #Map~get(#MASM~stack, #MASM~stackPtr);
    #MASM~stackPtr := #MASM~stackPtr - 8;
}

var #CacheIR~knownOperandIds: #Set #OperandId;

var #CacheIR~operandLocations: #Map #OperandId #OperandLocation;

procedure #CacheIR~defineReg($typedId: #TypedId)
    returns (reg: #Reg)
    modifies #CacheIR~operandLocations, #CacheIR~allocatedRegs;
{
    var $operandId'0: #OperandId;
    var $loc'0: #OperandLocation;
    var $loc'1: #OperandLocation;
    var $locKind'0: #OperandLocationKind;
    
    $operandId'0 := #TypedId^to#OperandId($typedId);
    $loc'0 := #Map~get(#CacheIR~operandLocations, $operandId'0);
    $locKind'0 := #OperandLocation~kind($loc'0);
    assert $locKind'0 == #OperandLocationKind^Variant~Uninitialized();
    
    call reg := #CacheIR~allocateReg();
    call $loc'1 := #OperandLocation~setPayloadReg(reg, #TypedId~type($typedId));
    #CacheIR~operandLocations := #Map~set(#CacheIR~operandLocations, $operandId'0, $loc'1);
}

procedure #CacheIR~defineValueReg($valueId: #ValueId)
    returns (reg: #ValueReg)
    modifies #CacheIR~operandLocations, #CacheIR~allocatedRegs;
{
    var $operandId'0: #OperandId;
    var $loc'0: #OperandLocation;
    var $loc'1: #OperandLocation;
    var $locKind'0: #OperandLocationKind;
    
    $operandId'0 := #ValueId^to#OperandId($valueId);
    $loc'0 := #Map~get(#CacheIR~operandLocations, $operandId'0);
    $locKind'0 := #OperandLocation~kind($loc'0);
    assert $locKind'0 == #OperandLocationKind^Variant~Uninitialized();
    
    call reg := #CacheIR~allocateValueReg();
    call $loc'1 := #OperandLocation~setValueReg(reg);
    #CacheIR~operandLocations := #Map~set(#CacheIR~operandLocations, $operandId'0, $loc'1);
}

procedure #CacheIR~getOperandLocation($operandId: #OperandId)
    returns (loc: #OperandLocation)
{
    loc := #Map~get(#CacheIR~operandLocations, $operandId);
}

procedure #CacheIR~setOperandLocation($operandId: #OperandId, $loc: #OperandLocation)
    modifies #CacheIR~operandLocations;
{
    #CacheIR~operandLocations := #Map~set(#CacheIR~operandLocations, $operandId, $loc);
}

procedure #initInputOperandLocation($typedId: #TypedId)
    modifies #CacheIR~operandLocations, #CacheIR~knownOperandIds;
{
    var $operandId'0: #OperandId;
    var tmp'0: #Bool;
    var $loc'0: #OperandLocation;

    $operandId'0 := #TypedId^to#OperandId($typedId);
    tmp'0 := #Set~contains(#CacheIR~knownOperandIds, $operandId'0);
    assume !tmp'0;
    #CacheIR~knownOperandIds := #Set~add(#CacheIR~knownOperandIds, $operandId'0);

    call $loc'0 := #OperandLocation~newUninitialized();
    #CacheIR~operandLocations := #Map~set(#CacheIR~operandLocations, $operandId'0, $loc'0);
}

procedure #initValueInputOperandLocation($valueId: #ValueId)
    modifies #CacheIR~operandLocations, #CacheIR~knownOperandIds;
{
    var $operandId'0: #OperandId;
    var tmp'0: #Bool;
    var $loc'0: #OperandLocation;

    $operandId'0 := #ValueId^to#OperandId($valueId);
    tmp'0 := #Set~contains(#CacheIR~knownOperandIds, $operandId'0);
    assume !tmp'0;
    #CacheIR~knownOperandIds := #Set~add(#CacheIR~knownOperandIds, $operandId'0);

    call $loc'0 := #OperandLocation~newUninitialized();
    #CacheIR~operandLocations := #Map~set(#CacheIR~operandLocations, $operandId'0, $loc'0);
}

procedure #initValueReg($valueReg: #ValueReg)
    modifies #MASM~regs;
{
    var $data'0: #RegData;
    var tmp'0: #Bool;

    call $data'0 := #MASM~getData($valueReg);
    call tmp'0 := #RegData~isValue($data'0);
    assume tmp'0;
}

var #CacheIR~allocatedRegs: #Set #Reg;

procedure #initAllocatedRegs()
  modifies #CacheIR~allocatedRegs;
{
  // initially all registers are unallocated
  assume (
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rax()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rbx()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rcx()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rdx()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rsp()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rbp()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rsi()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rdi()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R8()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R9()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R10()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R11()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R12()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R13()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R14()) &&
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R15())
  );
}

procedure #CacheIR~allocateValueReg()
  returns (ret: #ValueReg)
  modifies #CacheIR~allocatedRegs;
{
  call ret := #CacheIR~allocateReg();
}

procedure #CacheIR~releaseValueReg($valueReg: #ValueReg)
  modifies #CacheIR~allocatedRegs;
{
  call #CacheIR~releaseReg($valueReg);
}

procedure #CacheIR~allocateReg()
  returns (ret: #Reg)
  modifies #CacheIR~allocatedRegs;
{
  var $reg: #Reg;
  var tmp'0: #Bool;

  // ensure that we have enough registers by
  // asserting that there is atleast one allocatable
  // register that is not already allocated
  assert (
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rax()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rbx()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rcx()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rdx()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rsi()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~Rdi()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R8()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R9()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R10()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R12()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R13()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R14()) ||
    !#Set~contains(#CacheIR~allocatedRegs, #Reg^Variant~R15())
  );

  $reg := #CacheIR~allocateRegUnchecked(#CacheIR~allocatedRegs);

  // rsp, rbp and r11 are not allocatable registers
  assume (
    $reg != #Reg^Variant~Rsp() &&
    $reg != #Reg^Variant~Rbp() &&
    $reg != #Reg^Variant~R11()
  );

  tmp'0 := #Set~contains(#CacheIR~allocatedRegs, $reg);
  assume (
    (!tmp'0 && $reg == #Reg^Variant~Rax()) ||
    (!tmp'0 && $reg == #Reg^Variant~Rbx()) ||
    (!tmp'0 && $reg == #Reg^Variant~Rcx()) ||
    (!tmp'0 && $reg == #Reg^Variant~Rdx()) ||
    (!tmp'0 && $reg == #Reg^Variant~Rsi()) ||
    (!tmp'0 && $reg == #Reg^Variant~Rdi()) ||
    (!tmp'0 && $reg == #Reg^Variant~R8()) ||
    (!tmp'0 && $reg == #Reg^Variant~R9()) ||
    (!tmp'0 && $reg == #Reg^Variant~R10()) ||
    (!tmp'0 && $reg == #Reg^Variant~R12()) ||
    (!tmp'0 && $reg == #Reg^Variant~R13()) ||
    (!tmp'0 && $reg == #Reg^Variant~R14()) ||
    (!tmp'0 && $reg == #Reg^Variant~R15())
  );

  #CacheIR~allocatedRegs := #Set~add(#CacheIR~allocatedRegs, $reg);
  ret := $reg;
}

procedure #CacheIR~allocateKnownReg($reg: #Reg)
  modifies #CacheIR~allocatedRegs;
{
  // rsp, rbp and r11 are not allocatable registers
  assert (
    $reg != #Reg^Variant~Rsp() &&
    $reg != #Reg^Variant~Rbp() &&
    $reg != #Reg^Variant~R11()
  );

  // register should not already be allocated
  assert !#Set~contains(#CacheIR~allocatedRegs, $reg);

  #CacheIR~allocatedRegs := #Set~add(#CacheIR~allocatedRegs, $reg);
}

function #CacheIR~allocateRegUnchecked($allocatedRegs: #Set #Reg): #Reg;

procedure #CacheIR~releaseReg($reg: #Reg)
  modifies #CacheIR~allocatedRegs;
{
  // register should be allocated
  assert #Set~contains(#CacheIR~allocatedRegs, $reg);

  #CacheIR~allocatedRegs := #Set~remove(#CacheIR~allocatedRegs, $reg);
}

var #CacheIR~allocatedFloatRegs: #Set #FloatReg;

procedure #initAllocatedFloatRegs()
  modifies #CacheIR~allocatedFloatRegs;
{
  // initially all registers are unallocated
  assume (
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm0()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm1()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm2()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm3()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm4()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm5()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm6()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm7()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm8()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm9()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm10()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm11()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm12()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm13()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm14()) &&
    !#Set~contains(#CacheIR~allocatedFloatRegs, #FloatReg^Variant~Xmm15())
  );
}

procedure #CacheIR~allocateAvailableFloatReg($floatReg: #FloatReg)
  returns (ret: #FloatReg)
  modifies #CacheIR~allocatedFloatRegs;
{
  // xmm15 is not an allocatable register
  assert (
    $floatReg != #FloatReg^Variant~Xmm15()
  );

  call #CacheIR~allocateAvailableFloatRegUnchecked($floatReg: #FloatReg);
  ret := $floatReg;
}

procedure #CacheIR~allocateAvailableFloatRegUnchecked($floatReg: #FloatReg)
  modifies #CacheIR~allocatedFloatRegs;
{
  // register should not already be allocated
  assert !#Set~contains(#CacheIR~allocatedFloatRegs, $floatReg);

  #CacheIR~allocatedFloatRegs := #Set~add(#CacheIR~allocatedFloatRegs, $floatReg);
}

procedure #CacheIR~allocateFloatScratchReg()
  returns (ret: #FloatReg)
  modifies #CacheIR~allocatedFloatRegs;
{
  call #CacheIR~allocateAvailableFloatRegUnchecked(#FloatReg^Variant~Xmm15());
  ret := #FloatReg^Variant~Xmm15(); 
}

procedure #CacheIR~releaseFloatReg($floatReg: #FloatReg)
  modifies #CacheIR~allocatedFloatRegs;
{
  // register should be allocated
  assert #Set~contains(#CacheIR~allocatedFloatRegs, $floatReg);

  #CacheIR~allocatedFloatRegs := #Set~remove(#CacheIR~allocatedFloatRegs, $floatReg);
}

procedure #initRegAllocator()
  modifies #CacheIR~allocatedRegs, #CacheIR~allocatedFloatRegs;
{
    call #initAllocatedRegs();
    call #initAllocatedFloatRegs();
}
