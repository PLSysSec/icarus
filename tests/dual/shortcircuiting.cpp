#include <cassert>
#include <cstdlib>
#define Cachet_Assert assert
#define Cachet_Unreachable() abort()
#include <cpp_prelude.h>
#include <stdio.h>
using Cachet_ContextRef = std::monostate;

static int x = 0;

inline Type_Int32::MutRef Var_x(Cachet_ContextRef cx) {
    return x;
}

#include <test.h>
#include <test.inc>



int main() {
    std::monostate ctx;
    Fn_test(ctx);

    return 0;
}
