pragma circom 2.0.0;

// Regression test: a signal-dependent (dynamic) if/else whose branch is
// more than one instruction - specifically an assignment followed by a
// nested dynamic if/else, matching passport-zk-circuits' short_div_norm:
//   if (long_gt(...) == 1) {
//       mult = long_sub(...);
//       if (long_gt(...) == 1) { return qhat - 2; } else { return qhat - 1; }
//   } else {
//       return qhat;
//   }
// build-circuit's ternary-branch lowering only ever supported a branch
// that's exactly one instruction; this exercises the general multi-
// statement/nested-branch handling added to fix that.
function pick_nested(cond_outer, cond_inner, a, b) {
    var x;
    if (cond_outer != 0) {
        x = a + b;
        if (cond_inner != 0) {
            return x - 2;
        } else {
            return x - 1;
        }
    } else {
        return a;
    }
}

template Main() {
    signal input cond_outer;
    signal input cond_inner;
    signal input a;
    signal input b;
    signal output out;

    var result = pick_nested(cond_outer, cond_inner, a, b);
    out <-- result;
}

component main = Main();
