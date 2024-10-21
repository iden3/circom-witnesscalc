pragma circom 2.1.9;

include "bitify.circom";
include "multiplexer.circom";
include "gates.circom";

// Outputs a round constant for a given round number
template RCon(round) {
    signal output out[4];

    assert(round > 0 && round <= 10);

    var rcon[10][4] = [
        [0x01, 0x00, 0x00, 0x00],
        [0x02, 0x00, 0x00, 0x00],
        [0x04, 0x00, 0x00, 0x00],
        [0x08, 0x00, 0x00, 0x00],
        [0x10, 0x00, 0x00, 0x00],
        [0x20, 0x00, 0x00, 0x00],
        [0x40, 0x00, 0x00, 0x00],
        [0x80, 0x00, 0x00, 0x00],
        [0x1b, 0x00, 0x00, 0x00],
        [0x36, 0x00, 0x00, 0x00]
    ];

    out <== rcon[round-1];
}

// XORs two words (arrays of 4 bytes each)
template XorWord() {
    signal input bytes1[4];
    signal input bytes2[4];

    component n2b[4 * 2];
    component b2n[4];
    component xor[4][8];

    signal output out[4];

    for(var i = 0; i < 4; i++) {
        n2b[2 * i] = Num2Bits(8);
        n2b[2 * i + 1] = Num2Bits(8);
        n2b[2 * i].in <== bytes1[i];
        n2b[2 * i + 1].in <== bytes2[i];
        b2n[i] = Bits2Num(8);

        for (var j = 0; j < 8; j++) {
            xor[i][j] = XOR();
            xor[i][j].a <== n2b[2 * i].out[j];
            xor[i][j].b <== n2b[2 * i + 1].out[j];
            b2n[i].in[j] <== xor[i][j].out;
        }

        out[i] <== b2n[i].out;
    }
}

// AffineTransform required by the S-box computation.
template AffineTransform() {
    signal input inBits[8];
    signal output outBits[8];

    var matrix[8][8] = [[1, 0, 0, 0, 1, 1, 1, 1],
                        [1, 1, 0, 0, 0, 1, 1, 1],
                        [1, 1, 1, 0, 0, 0, 1, 1],
                        [1, 1, 1, 1, 0, 0, 0, 1],
                        [1, 1, 1, 1, 1, 0, 0, 0],
                        [0, 1, 1, 1, 1, 1, 0, 0],
                        [0, 0, 1, 1, 1, 1, 1, 0],
                        [0, 0, 0, 1, 1, 1, 1, 1]];
    var offset[8] = [1, 1, 0, 0, 0, 1, 1, 0];
    for (var i = 0; i < 8; i++) {
        var lc = 0;
        for (var j = 0; j < 8; j++) {
            if (matrix[i][j] == 1) {
                lc += inBits[j];
            }
        }
        lc += offset[i];
        outBits[i] <== IsOdd(3)(lc);
    }
}

// Determine the parity (odd or even) of an integer that can be accommodated within 'nBits' bits.
template IsOdd(nBits) {
    signal input in;
    signal output out;
    if (nBits == 1) {
        out <== in;
    } else {
        signal bits[nBits] <== Num2Bits(nBits)(in);
        out <== bits[0];
    }
}

// Finite field multiplication.
template FieldMul() {
    signal input a;
    signal input b;
    signal inBits[2][8];
    signal output out;

    inBits[0] <== Num2Bits(8)(a);
    inBits[1] <== Num2Bits(8)(b);

    // List of finite field elements obtained by successively doubling, starting from 1.
    var power[15] = [0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36, 0x6c, 0xd8, 0xab, 0x4d, 0x9a];

    signal mulMatrix[8][8];
    var outLinesLc[8];
    for (var i = 0; i < 8; i++) {
        outLinesLc[i] = 0;
    }
    // Apply elementary multiplication
    for (var i = 0; i < 8; i++) {
        for (var j = 0; j < 8; j++) {
            mulMatrix[i][j] <== inBits[0][i] * inBits[1][j];
            for (var t = 0; t < 8; t++) {
                if (power[i+j] & (1 << t) != 0) {
                    outLinesLc[t] += mulMatrix[i][j];
                }
            }
        }
    }
    signal outBitsUnreduced[8];
    signal outBits[8];
    for (var i = 0; i < 8; i++) {
        outBitsUnreduced[i] <== outLinesLc[i];
        // Each element in 'outLinesLc' is incremented by a known constant number of
        // elements from 'mulMatrix', less than 31.
        outBits[i] <== IsOdd(6)(outBitsUnreduced[i]);
    }

    out <== Bits2Num(8)(outBits);
}

// Finite Field Inversion. Specially, if the input is 0, the output is also 0.
template FieldInv() {
    signal input in;
    signal output out;

    var inv[256] = [0x00, 0x01, 0x8d, 0xf6, 0xcb, 0x52, 0x7b, 0xd1, 0xe8, 0x4f, 0x29, 0xc0, 0xb0, 0xe1, 0xe5, 0xc7,
                    0x74, 0xb4, 0xaa, 0x4b, 0x99, 0x2b, 0x60, 0x5f, 0x58, 0x3f, 0xfd, 0xcc, 0xff, 0x40, 0xee, 0xb2,
                    0x3a, 0x6e, 0x5a, 0xf1, 0x55, 0x4d, 0xa8, 0xc9, 0xc1, 0x0a, 0x98, 0x15, 0x30, 0x44, 0xa2, 0xc2,
                    0x2c, 0x45, 0x92, 0x6c, 0xf3, 0x39, 0x66, 0x42, 0xf2, 0x35, 0x20, 0x6f, 0x77, 0xbb, 0x59, 0x19,
                    0x1d, 0xfe, 0x37, 0x67, 0x2d, 0x31, 0xf5, 0x69, 0xa7, 0x64, 0xab, 0x13, 0x54, 0x25, 0xe9, 0x09,
                    0xed, 0x5c, 0x05, 0xca, 0x4c, 0x24, 0x87, 0xbf, 0x18, 0x3e, 0x22, 0xf0, 0x51, 0xec, 0x61, 0x17,
                    0x16, 0x5e, 0xaf, 0xd3, 0x49, 0xa6, 0x36, 0x43, 0xf4, 0x47, 0x91, 0xdf, 0x33, 0x93, 0x21, 0x3b,
                    0x79, 0xb7, 0x97, 0x85, 0x10, 0xb5, 0xba, 0x3c, 0xb6, 0x70, 0xd0, 0x06, 0xa1, 0xfa, 0x81, 0x82,
                    0x83, 0x7e, 0x7f, 0x80, 0x96, 0x73, 0xbe, 0x56, 0x9b, 0x9e, 0x95, 0xd9, 0xf7, 0x02, 0xb9, 0xa4,
                    0xde, 0x6a, 0x32, 0x6d, 0xd8, 0x8a, 0x84, 0x72, 0x2a, 0x14, 0x9f, 0x88, 0xf9, 0xdc, 0x89, 0x9a,
                    0xfb, 0x7c, 0x2e, 0xc3, 0x8f, 0xb8, 0x65, 0x48, 0x26, 0xc8, 0x12, 0x4a, 0xce, 0xe7, 0xd2, 0x62,
                    0x0c, 0xe0, 0x1f, 0xef, 0x11, 0x75, 0x78, 0x71, 0xa5, 0x8e, 0x76, 0x3d, 0xbd, 0xbc, 0x86, 0x57,
                    0x0b, 0x28, 0x2f, 0xa3, 0xda, 0xd4, 0xe4, 0x0f, 0xa9, 0x27, 0x53, 0x04, 0x1b, 0xfc, 0xac, 0xe6,
                    0x7a, 0x07, 0xae, 0x63, 0xc5, 0xdb, 0xe2, 0xea, 0x94, 0x8b, 0xc4, 0xd5, 0x9d, 0xf8, 0x90, 0x6b,
                    0xb1, 0x0d, 0xd6, 0xeb, 0xc6, 0x0e, 0xcf, 0xad, 0x08, 0x4e, 0xd7, 0xe3, 0x5d, 0x50, 0x1e, 0xb3,
                    0x5b, 0x23, 0x38, 0x34, 0x68, 0x46, 0x03, 0x8c, 0xdd, 0x9c, 0x7d, 0xa0, 0xcd, 0x1a, 0x41, 0x1c];

    component mux = Multiplexer(1, 256);
    for (var i = 0; i < 256; i++) {
        mux.inp[i][0] <== inv[i];
    }
    mux.sel <== in;

    // Obtain an unchecked result from a lookup table
    // out <-- inv[in];
    out <== mux.out[0];
    // Compute the product of the input and output, expected to be 1
    signal checkRes <== FieldMul()(in, out);
    // For the special case when the input is 0, both input and output should be 0
    signal isZeroIn <== IsZero()(in);
    signal isZeroOut <== IsZero()(out);
    signal checkZero <== isZeroIn * isZeroOut;
    // Ensure that either the product is 1 or both input and output are 0, satisfying at least one condition
    (1 - checkRes) * (1 - checkZero) === 0;
}

template SBox128() {
    signal input in;
    signal output out;

    signal inv <== FieldInv()(in);
    signal invBits[8] <== Num2Bits(8)(inv);
    signal outBits[8] <== AffineTransform()(invBits);
    out <== Bits2Num(8)(outBits);
}

// Substitutes each byte in a word using the S-Box
template SubstituteWord() {
    signal input bytes[4];
    signal output substituted[4];

    component sbox[4];

    for(var i = 0; i < 4; i++) {
        sbox[i] = SBox128();
        sbox[i].in <== bytes[i];
        substituted[i] <== sbox[i].out;
    }
}

// Rotates an array of bytes to the left by a specified rotation
template Rotate(rotation, length) {
    assert(rotation < length);
    signal input bytes[length];
    signal output rotated[length];

    for(var i = 0; i < length - rotation; i++) {
        rotated[i] <== bytes[i + rotation];
    }

    for(var i = length - rotation; i < length; i++) {
        rotated[i] <== bytes[i - length + rotation];
    }
}

// @param nk: number of keys which can be 4, 6, 8
// @param o: number of output words which can be 4 or nk
template NextRound(nk, o, round){
    signal input key[nk][4];
    signal output nextKey[o][4];

    component rotateWord = Rotate(1, 4);
    for (var i = 0; i < 4; i++) {
        rotateWord.bytes[i] <== key[nk - 1][i];
    }

    component substituteWord[2];
    substituteWord[0] = SubstituteWord();
    substituteWord[0].bytes <== rotateWord.rotated;

    component rcon = RCon(round);

    component xorWord[o + 1];
    xorWord[0] = XorWord();
    xorWord[0].bytes1 <== substituteWord[0].substituted;
    xorWord[0].bytes2 <== rcon.out;

    for (var i = 0; i < o; i++) {
        xorWord[i+1] = XorWord();
        if (i == 0) {
            xorWord[i+1].bytes1 <== xorWord[0].out;
        } else if(nk == 8 && i == 4) {
            substituteWord[1] = SubstituteWord();
            substituteWord[1].bytes <== nextKey[i - 1];
            xorWord[i+1].bytes1 <== substituteWord[1].substituted;
        } else {
            xorWord[i+1].bytes1 <== nextKey[i-1];
        }
        xorWord[i+1].bytes2 <== key[i];

        for (var j = 0; j < 4; j++) {
            nextKey[i][j] <== xorWord[i+1].out[j];
        }
    }
}


// @param nk: number of keys which can be 4, 6, 8
// @param nr: number of rounds which can be 10, 12, 14 for AES 128, 192, 256
// @inputs key: array of nk*4 bytes representing the key
// @outputs keyExpanded: array of (nr+1)*4 words i.e for AES 128, 192, 256 it will be 44, 52, 60 words
template KeyExpansion(nk,nr) {
    assert(nk == 4 || nk == 6 || nk == 8 );
    signal input key[nk * 4];

    var totalWords = (4 * (nr + 1));
    var effectiveRounds = nk == 4 ? 10 : totalWords\nk;

    signal output keyExpanded[totalWords][4];

    for (var i = 0; i < nk; i++) {
        for (var j = 0; j < 4; j++) {
            keyExpanded[i][j] <== key[(4 * i) + j];
        }
    }

    component nextRound[effectiveRounds];

    for (var round = 1; round <= effectiveRounds; round++) {
        var outputWordLen = round == effectiveRounds ? 4 : nk;
        nextRound[round - 1] = NextRound(nk, outputWordLen, round);

        for (var i = 0; i < nk; i++) {
            for (var j = 0; j < 4; j++) {
                nextRound[round - 1].key[i][j] <== keyExpanded[(round * nk) + i - nk][j];
            }
        }

        for (var i = 0; i < outputWordLen; i++) {
            for (var j = 0; j < 4; j++) {
                keyExpanded[(round * nk) + i][j] <== nextRound[round - 1].nextKey[i][j];
            }
        }
    }
}

component main = KeyExpansion(4, 10);
