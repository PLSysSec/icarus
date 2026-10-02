#include <cassert>
#include <stdio.h>
#include <iostream>

#define Cachet_Assert assert
#define Cachet_Unreachable() abort()

#include <cpp_prelude.h>

using Cachet_ContextRef = std::monostate;

struct Range {
    int32_t lower_;
    int32_t upper_;
    bool hasInt32LowerBound_;
    bool hasInt32UpperBound_;
    bool canHaveFractionalPart_;
    bool canBeNegativeZero_;
    uint16_t max_exponent_;
};

std::ostream& operator<<(std::ostream& os, const Range& range) {
    return os << "lower_: " << range.lower_ << std::endl
              << "upper_: " << range.upper_ << std::endl
              << "hasInt32LowerBound_: " << range.hasInt32LowerBound_ << std::endl
              << "hasInt32UpperBound_:" << range.hasInt32UpperBound_ << std::endl
              << "canHaveFractionalPart_:" << range.canHaveFractionalPart_ << std::endl
              << "canBeNegativeZero_:" << range.canBeNegativeZero_ << std::endl
              << "max_exponent_:" << range.max_exponent_ << std::endl;
}


using JSVal = double;
using Type_JSVal = PrimitiveType<JSVal*>;
using Type_Range = PrimitiveType<Range*>;

Type_Range::Val Fn_mk_range_raw(
    std::monostate ctx,
    int32_t lower_,
    int32_t upper_,
    bool hasInt32LowerBound_,
    bool hasInt32UpperBound_,
    bool canHaveFractionalPart_,
    bool canBeNegativeZero_,
    uint16_t max_exponent_
) {
    Range* r = new Range();
    r->lower_ = lower_;
    r->upper_ = upper_;
    r->hasInt32LowerBound_ = hasInt32LowerBound_;
    r->hasInt32UpperBound_ = hasInt32UpperBound_;
    r->canBeNegativeZero_ = canBeNegativeZero_;
    r->canHaveFractionalPart_ = canHaveFractionalPart_;
    r->max_exponent_ = max_exponent_;
    return r;
}

inline Type_Double::Val Var_x(std::monostate ctx) {
    return -2;
}

inline Range* Var_r(std::monostate ctx) {
    return Fn_mk_range_raw(
        ctx,
        -3,
        8,
        true,
        true,
        false,
        true,
        3
    );
}

#include <test.h>
#include <test.inc>

int main() {
    std::monostate ctx;
    Fn_test(ctx);
    return 0;
}
