pragma circom 2.0.0;

// Regression test: a subcomponent output array signal where only some
// indices are ever assigned (e.g. a recursive template's base case that
// writes out[0] but never out[1] on that particular instantiation,
// matching passport-zk-circuits' KaratsubaOverflow(1)). Real circom's own
// witness generators leave the untouched index at its buffer default (0);
// witness-graph construction must do the same on both the single-signal
// and array-load paths, instead of panicking on a "signal not set" read.
template Leaf() {
    signal input a;
    signal input b;
    signal output out[2];

    out[0] <== a * b;
    // out[1] intentionally left unassigned
}

template Main() {
    signal input a;
    signal input b;
    signal output single;
    signal output bulk[2];

    component leaf1 = Leaf();
    leaf1.a <== a;
    leaf1.b <== b;
    single <== leaf1.out[0] + leaf1.out[1];

    component leaf2 = Leaf();
    leaf2.a <== a;
    leaf2.b <== b;
    bulk <== leaf2.out;
}

component main = Main();
