pragma circom 2.0.0;

// Regression test: a subcomponent input array sized 0 is a legitimate,
// no-op circom construct (e.g. a passport-verification circuit variant
// with no DG15/active-authentication support declares
// `signal input dg15[0]`). Storing a zero-length array signal into it via
// `<==` must not panic during witness-graph construction.
template Sub(N) {
    signal input arr[N];
    signal output out;

    var sum = 0;
    for (var i = 0; i < N; i++) {
        sum += arr[i];
    }
    out <== sum;
}

template Main() {
    signal input x;
    signal input emptyArr[0];
    signal output out;

    component sub = Sub(0);
    sub.arr <== emptyArr;

    out <== x + sub.out;
}

component main = Main();
