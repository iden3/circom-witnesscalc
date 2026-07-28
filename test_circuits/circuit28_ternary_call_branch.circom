pragma circom 2.0.0;

// Regression test: a signal-dependent (dynamic) `if`/`else` inside a
// circom `function`, where each branch's single statement is a function
// call rather than a plain expression (e.g. passport-zk-circuits'
// `short_div`: `if (norm_b[k] != 0) { ret = short_div_norm(...); } else
// { ret = short_div_norm(...); }`, both branches calling a helper
// function). This exercises the ternary-branch lowering used when the
// condition can't be resolved to a compile-time constant.
//
// Note: `out` uses `<--` rather than `<==` because real circom's own
// degree checker (T3001 "non quadratic constraints") conservatively
// rejects binding a value returned from a function containing a dynamic
// ternary directly into a `<==`, regardless of the value's actual
// algebraic degree - a pre-existing circom front-end quirk, unrelated to
// build-circuit. `<--` reproduces the ternary/Call defect this commit
// fixes (both branches run to completion instead of panicking on
// "expected store operation in ternary operation") and then separately
// hits build-circuit's pre-existing, distinct `todo!("Signal")` in
// store_function_return_results for AddressType::Signal - a real gap,
// but not this bug; not attempted here.
function double(x) {
    return x * 2;
}

function triple(x) {
    return x * 3;
}

function pick(cond, x) {
    var ret;
    if (cond != 0) {
        ret = double(x);
    } else {
        ret = triple(x);
    }
    return ret;
}

template Main() {
    signal input cond;
    signal output out;

    out <-- pick(cond, 5);
}

component main = Main();
