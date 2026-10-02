#include <cassert>
#include <cstdlib>

#define Cachet_Assert assert
#define Cachet_Unreachable() abort()

#include <cpp_prelude.h>
#include <stdio.h>

using Cachet_ContextRef = std::monostate;

struct Foo {
    int32_t foo;
};

using Type_Foo = PrimitiveType<Foo*>;

#include <test.h>
#include <test.inc>

int main() {
    std::monostate ctx;
    Foo bar { 15 };
    return Fn_test(ctx, &bar) == 15 ? 0 : 1;
}
