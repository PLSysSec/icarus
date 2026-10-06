#include <variant>
#include <cassert>
#include <cstdlib>

#define Cachet_Assert assert
#define Cachet_Unreachable() abort()

using Cachet_ContextRef = std::monostate;


#include <cpp_prelude.h>

#include <test.h>
#include <test.inc>

int main () {
    Fn_test(std::monostate());
}