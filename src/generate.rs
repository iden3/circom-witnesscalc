#![allow(non_snake_case)]

use crate::field::{self, *};
use crate::graph::{self, Node};
use crate::HashSignalInfo;
use byteorder::{LittleEndian, ReadBytesExt};
use ffi::InputOutputList;
use ruint::{aliases::U256, uint};
use serde::{Deserialize, Serialize};
use std::{io::Read, time::Instant};
use std::collections::HashMap;

#[derive(Debug)]
pub struct IOSignalsMap {
    pub m: HashMap<u32, InputOutputList>,
}

#[cxx::bridge]
mod ffi {

    #[derive(Debug, Default, Clone)]
    pub struct InputOutputList {
        pub defs: Vec<IODef>,
    }

    #[derive(Debug, Clone, Default)]
    pub struct IODef {
        pub code: usize,
        pub offset: usize,
        pub lengths: Vec<usize>,
    }

    #[derive(Debug, Default, Clone)]
    struct Circom_Component {
        templateId: u64,
        signalStart: u64,
        inputCounter: u64,
        templateName: String,
        componentName: String,
        idFather: u64,
        subcomponents: Vec<u32>,
        outputIsSet: Vec<bool>,
    }

    #[derive(Debug)]
    struct Circom_CalcWit<'a> {
        signalValues: Vec<FrElement>,
        componentMemory: Vec<Circom_Component>,
        circuitConstants: Vec<FrElement>,
        templateInsId2IOSignalInfoList: &'a IOSignalsMap,
        listOfTemplateMessages: Vec<String>,
    }

    // Rust types and signatures exposed to C++.
    extern "Rust" {
        type FrElement;
        type IOSignalsMap;

        fn create_vec(len: usize) -> Vec<FrElement>;
        fn create_vec_u32(len: usize) -> Vec<u32>;
        fn generate_position_array(
            prefix: String,
            dimensions: Vec<u32>,
            size_dimensions: u32,
            index: u32,
        ) -> String;

        // Field operations
        unsafe fn Fr_mul(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_add(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_sub(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_copy(to: *mut FrElement, a: *const FrElement);
        unsafe fn Fr_copyn(to: *mut FrElement, a: *const FrElement, n: usize);
        unsafe fn Fr_neg(to: *mut FrElement, a: *const FrElement);
        // unsafe fn Fr_inv(to: *mut FrElement, a: *const FrElement);
        unsafe fn Fr_div(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        // unsafe fn Fr_square(to: *mut FrElement, a: *const FrElement);
        unsafe fn Fr_shl(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_shr(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_band(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        // fn Fr_bor(to: &mut FrElement, a: &FrElement, b: &FrElement);
        // fn Fr_bxor(to: &mut FrElement, a: &FrElement, b: &FrElement);
        // fn Fr_bnot(to: &mut FrElement, a: &FrElement);
        unsafe fn Fr_eq(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_neq(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_lt(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_gt(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_leq(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_geq(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_isTrue(a: *mut FrElement) -> bool;
        // fn Fr_fromBool(to: &mut FrElement, a: bool);
        unsafe fn Fr_toInt(a: *mut FrElement) -> u64;
        unsafe fn Fr_land(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn Fr_lor(to: *mut FrElement, a: *const FrElement, b: *const FrElement);
        unsafe fn print(a: *mut FrElement);
        // fn Fr_pow(to: &mut FrElement, a: &FrElement, b: &FrElement);
        unsafe fn Fr_idiv(to: *mut FrElement, a: *const FrElement, b: *const FrElement);

        // By pointer to FrElement return an index of the element
        // in the VALUES vector
        unsafe fn get_element_index(a: *mut FrElement) -> usize;

        // By pointer to FrElement return the BigInt value in a string
        // representation of the element
        unsafe fn get_value(a: *const FrElement) -> String;

        fn get_signal_info(x: &IOSignalsMap, idx: u32) -> InputOutputList;
    }

    // C++ types and signatures exposed to Rust.
    unsafe extern "C++" {
        include!("witness/include/witness.h");

        unsafe fn run(ctx: *mut Circom_CalcWit);
        fn get_size_of_io_map() -> u32;
        fn get_total_signal_no() -> u32;
        fn get_main_input_signal_no() -> u32;
        fn get_main_input_signal_start() -> u32;
        fn get_number_of_components() -> u32;
        fn get_size_of_constants() -> u32;
        fn get_size_of_input_hashmap() -> u32;
        fn get_size_of_witness() -> u32;
    }
}

const DAT_BYTES: &[u8] = include_bytes!("constants.dat");

pub fn get_input_hash_map() -> Vec<HashSignalInfo> {
    let mut bytes = &DAT_BYTES[..(ffi::get_size_of_input_hashmap() as usize) * 24];
    let mut input_hash_map =
        vec![HashSignalInfo::default(); ffi::get_size_of_input_hashmap() as usize];
    for i in 0..ffi::get_size_of_input_hashmap() as usize {
        let hash = bytes.read_u64::<LittleEndian>().unwrap();
        let signalid = bytes.read_u64::<LittleEndian>().unwrap();
        let signalsize = bytes.read_u64::<LittleEndian>().unwrap();
        input_hash_map[i] = HashSignalInfo {
            hash,
            signalid,
            signalsize,
        };
    }
    input_hash_map
}

pub fn get_witness_to_signal() -> Vec<usize> {
    let mut bytes = &DAT_BYTES[(ffi::get_size_of_input_hashmap() as usize) * 24
        ..(ffi::get_size_of_input_hashmap() as usize) * 24
            + (ffi::get_size_of_witness() as usize) * 8];
    let mut signal_list = Vec::with_capacity(ffi::get_size_of_witness() as usize);
    for i in 0..ffi::get_size_of_witness() as usize {
        signal_list.push(bytes.read_u64::<LittleEndian>().unwrap() as usize);
    }
    signal_list
}

pub fn get_constants() -> Vec<FrElement> {
    if ffi::get_size_of_constants() == 0 {
        return vec![];
    }

    // skip the first part
    let mut bytes = &DAT_BYTES[(ffi::get_size_of_input_hashmap() as usize) * 24
        + (ffi::get_size_of_witness() as usize) * 8..];
    let mut constants = vec![field::constant(U256::from(0)); ffi::get_size_of_constants() as usize];
    for i in 0..ffi::get_size_of_constants() as usize {
        let sv = bytes.read_i32::<LittleEndian>().unwrap();
        let typ = bytes.read_u32::<LittleEndian>().unwrap();

        let mut buf = [0; 32];
        bytes.read_exact(&mut buf).expect("read_exact failed");

        if typ & 0x80000000 == 0 {
            if sv >= 0 {
                constants[i] = constant(U256::from(sv));
            } else {
                constants[i] = constant(M - U256::from(sv*-1));
            }
        } else {
            constants[i] =
                constant(U256::from_le_bytes(buf).mul_redc(uint!(1_U256), M, INV));
        }
    }

    return constants;
}

pub fn get_iosignals() -> HashMap<u32, InputOutputList> {
    let io_size = ffi::get_size_of_io_map() as usize;
    let mut result: HashMap<u32, InputOutputList> = HashMap::with_capacity(io_size);

    if ffi::get_size_of_io_map() == 0 {
        return result;
    }

    // skip the first part
    let mut bytes = &DAT_BYTES[(ffi::get_size_of_input_hashmap() as usize) * 24
        + (ffi::get_size_of_witness() as usize) * 8
        + (ffi::get_size_of_constants() as usize * 40)..];
    let mut indices = vec![0u32; io_size];

    (0..io_size).for_each(|i| {
        indices[i] = bytes.read_u32::<LittleEndian>().unwrap();
    });

    let mut result: HashMap<u32, InputOutputList> = HashMap::with_capacity(io_size);

    (0..io_size).for_each(|i| {
        let l32 = bytes.read_u32::<LittleEndian>().unwrap() as usize;
        let mut io_list: InputOutputList = InputOutputList { defs: vec![] };

        (0..l32).for_each(|_j| {
            let offset = bytes.read_u32::<LittleEndian>().unwrap() as usize;
            let len = bytes.read_u32::<LittleEndian>().unwrap() as usize + 1;

            let mut lengths = vec![0usize; len];

            (1..len).for_each(|k| {
                lengths[k] = bytes.read_u32::<LittleEndian>().unwrap() as usize;
            });

            io_list.defs.push(ffi::IODef {
                code: 0,
                offset,
                lengths,
            });
        });
        result.insert(indices[i], io_list);
    });
    result
}

fn set_authV2_signals(signal_values: &mut Vec<FrElement>) {
    signal_values[2] = field::input(2, uint!(12345_U256));
    signal_values[3] = field::input(3, uint!(1243904711429961858774220647610724273798918457991486031567244100767259239747_U256));
    signal_values[4] = field::input(4, uint!(23148936466334350744548790012294489365207440754509988986684797708370051073_U256));
    signal_values[5] = field::input(5, uint!(10_U256));
    signal_values[6] = field::input(6, uint!(8039964009611210398788855768060749920589777058607598891238307089541758339342_U256));
    signal_values[7] = field::input(7, uint!(8162166103065016664685834856644195001371303013149727027131225893397958846382_U256));
    signal_values[8] = field::input(8, uint!(0_U256));
    signal_values[9] = field::input(9, uint!(0_U256));
    signal_values[10] = field::input(10, uint!(80551937543569765027552589160822318028_U256));
    signal_values[11] = field::input(11, uint!(0_U256));
    signal_values[12] = field::input(12, uint!(4720763745722683616702324599137259461509439547324750011830105416383780791263_U256));
    signal_values[13] = field::input(13, uint!(4844030361230692908091131578688419341633213823133966379083981236400104720538_U256));
    signal_values[14] = field::input(14, uint!(16547485850637761685_U256));
    signal_values[15] = field::input(15, uint!(0_U256));
    signal_values[16] = field::input(16, uint!(0_U256));
    signal_values[17] = field::input(17, uint!(0_U256));
    signal_values[18] = field::input(18, uint!(0_U256));
    signal_values[19] = field::input(19, uint!(0_U256));
    signal_values[20] = field::input(20, uint!(0_U256));
    signal_values[21] = field::input(21, uint!(0_U256));
    signal_values[22] = field::input(22, uint!(0_U256));
    signal_values[23] = field::input(23, uint!(0_U256));
    signal_values[24] = field::input(24, uint!(0_U256));
    signal_values[25] = field::input(25, uint!(0_U256));
    signal_values[26] = field::input(26, uint!(0_U256));
    signal_values[27] = field::input(27, uint!(0_U256));
    signal_values[28] = field::input(28, uint!(0_U256));
    signal_values[29] = field::input(29, uint!(0_U256));
    signal_values[30] = field::input(30, uint!(0_U256));
    signal_values[31] = field::input(31, uint!(0_U256));
    signal_values[32] = field::input(32, uint!(0_U256));
    signal_values[33] = field::input(33, uint!(0_U256));
    signal_values[34] = field::input(34, uint!(0_U256));
    signal_values[35] = field::input(35, uint!(0_U256));
    signal_values[36] = field::input(36, uint!(0_U256));
    signal_values[37] = field::input(37, uint!(0_U256));
    signal_values[38] = field::input(38, uint!(0_U256));
    signal_values[39] = field::input(39, uint!(0_U256));
    signal_values[40] = field::input(40, uint!(0_U256));
    signal_values[41] = field::input(41, uint!(0_U256));
    signal_values[42] = field::input(42, uint!(0_U256));
    signal_values[43] = field::input(43, uint!(0_U256));
    signal_values[44] = field::input(44, uint!(0_U256));
    signal_values[45] = field::input(45, uint!(0_U256));
    signal_values[46] = field::input(46, uint!(0_U256));
    signal_values[47] = field::input(47, uint!(0_U256));
    signal_values[48] = field::input(48, uint!(0_U256));
    signal_values[49] = field::input(49, uint!(0_U256));
    signal_values[50] = field::input(50, uint!(0_U256));
    signal_values[51] = field::input(51, uint!(0_U256));
    signal_values[52] = field::input(52, uint!(0_U256));
    signal_values[53] = field::input(53, uint!(0_U256));
    signal_values[54] = field::input(54, uint!(0_U256));
    signal_values[55] = field::input(55, uint!(0_U256));
    signal_values[56] = field::input(56, uint!(0_U256));
    signal_values[57] = field::input(57, uint!(0_U256));
    signal_values[58] = field::input(58, uint!(0_U256));
    signal_values[59] = field::input(59, uint!(0_U256));
    signal_values[60] = field::input(60, uint!(0_U256));
    signal_values[61] = field::input(61, uint!(0_U256));
    signal_values[62] = field::input(62, uint!(0_U256));
    signal_values[63] = field::input(63, uint!(0_U256));
    signal_values[64] = field::input(64, uint!(0_U256));
    signal_values[65] = field::input(65, uint!(0_U256));
    signal_values[66] = field::input(66, uint!(0_U256));
    signal_values[67] = field::input(67, uint!(0_U256));
    signal_values[68] = field::input(68, uint!(0_U256));
    signal_values[69] = field::input(69, uint!(0_U256));
    signal_values[70] = field::input(70, uint!(0_U256));
    signal_values[71] = field::input(71, uint!(0_U256));
    signal_values[72] = field::input(72, uint!(0_U256));
    signal_values[73] = field::input(73, uint!(0_U256));
    signal_values[74] = field::input(74, uint!(0_U256));
    signal_values[75] = field::input(75, uint!(0_U256));
    signal_values[76] = field::input(76, uint!(0_U256));
    signal_values[77] = field::input(77, uint!(0_U256));
    signal_values[78] = field::input(78, uint!(0_U256));
    signal_values[79] = field::input(79, uint!(0_U256));
    signal_values[80] = field::input(80, uint!(0_U256));
    signal_values[81] = field::input(81, uint!(0_U256));
    signal_values[82] = field::input(82, uint!(0_U256));
    signal_values[83] = field::input(83, uint!(0_U256));
    signal_values[84] = field::input(84, uint!(0_U256));
    signal_values[85] = field::input(85, uint!(0_U256));
    signal_values[86] = field::input(86, uint!(0_U256));
    signal_values[87] = field::input(87, uint!(0_U256));
    signal_values[88] = field::input(88, uint!(0_U256));
    signal_values[89] = field::input(89, uint!(0_U256));
    signal_values[90] = field::input(90, uint!(0_U256));
    signal_values[91] = field::input(91, uint!(0_U256));
    signal_values[92] = field::input(92, uint!(0_U256));
    signal_values[93] = field::input(93, uint!(0_U256));
    signal_values[94] = field::input(94, uint!(0_U256));
    signal_values[95] = field::input(95, uint!(0_U256));
    signal_values[96] = field::input(96, uint!(0_U256));
    signal_values[97] = field::input(97, uint!(0_U256));
    signal_values[98] = field::input(98, uint!(1_U256));
    signal_values[99] = field::input(99, uint!(0_U256));
    signal_values[100] = field::input(100, uint!(0_U256));
    signal_values[101] = field::input(101, uint!(15829360093371098546177008474519342171461782120259125067189481965541223738777_U256));
    signal_values[102] = field::input(102, uint!(10840522802382821290541462398953040493080116495308402635486440290351677745960_U256));
    signal_values[103] = field::input(103, uint!(1196477404779941775725836688033485533497812196897664950083199167075327114562_U256));
    signal_values[104] = field::input(104, uint!(0_U256));
    signal_values[105] = field::input(105, uint!(0_U256));
    signal_values[106] = field::input(106, uint!(0_U256));
    signal_values[107] = field::input(107, uint!(0_U256));
    signal_values[108] = field::input(108, uint!(0_U256));
    signal_values[109] = field::input(109, uint!(0_U256));
    signal_values[110] = field::input(110, uint!(0_U256));
    signal_values[111] = field::input(111, uint!(0_U256));
    signal_values[112] = field::input(112, uint!(0_U256));
    signal_values[113] = field::input(113, uint!(0_U256));
    signal_values[114] = field::input(114, uint!(0_U256));
    signal_values[115] = field::input(115, uint!(0_U256));
    signal_values[116] = field::input(116, uint!(0_U256));
    signal_values[117] = field::input(117, uint!(0_U256));
    signal_values[118] = field::input(118, uint!(0_U256));
    signal_values[119] = field::input(119, uint!(0_U256));
    signal_values[120] = field::input(120, uint!(0_U256));
    signal_values[121] = field::input(121, uint!(0_U256));
    signal_values[122] = field::input(122, uint!(0_U256));
    signal_values[123] = field::input(123, uint!(0_U256));
    signal_values[124] = field::input(124, uint!(0_U256));
    signal_values[125] = field::input(125, uint!(0_U256));
    signal_values[126] = field::input(126, uint!(0_U256));
    signal_values[127] = field::input(127, uint!(0_U256));
    signal_values[128] = field::input(128, uint!(0_U256));
    signal_values[129] = field::input(129, uint!(0_U256));
    signal_values[130] = field::input(130, uint!(0_U256));
    signal_values[131] = field::input(131, uint!(0_U256));
    signal_values[132] = field::input(132, uint!(0_U256));
    signal_values[133] = field::input(133, uint!(0_U256));
    signal_values[134] = field::input(134, uint!(0_U256));
    signal_values[135] = field::input(135, uint!(0_U256));
    signal_values[136] = field::input(136, uint!(0_U256));
    signal_values[137] = field::input(137, uint!(0_U256));
    signal_values[138] = field::input(138, uint!(0_U256));
    signal_values[139] = field::input(139, uint!(0_U256));
    signal_values[140] = field::input(140, uint!(0_U256));
    signal_values[141] = field::input(141, uint!(0_U256));
    signal_values[142] = field::input(142, uint!(0_U256));
    signal_values[143] = field::input(143, uint!(0_U256));
    signal_values[144] = field::input(144, uint!(0_U256));
    signal_values[145] = field::input(145, uint!(0_U256));
    signal_values[146] = field::input(146, uint!(0_U256));
    signal_values[147] = field::input(147, uint!(0_U256));
    signal_values[148] = field::input(148, uint!(0_U256));
    signal_values[149] = field::input(149, uint!(0_U256));
    signal_values[150] = field::input(150, uint!(0_U256));
    signal_values[151] = field::input(151, uint!(0_U256));
    signal_values[152] = field::input(152, uint!(0_U256));
    signal_values[153] = field::input(153, uint!(0_U256));
    signal_values[154] = field::input(154, uint!(0_U256));
    signal_values[155] = field::input(155, uint!(0_U256));
    signal_values[156] = field::input(156, uint!(0_U256));
    signal_values[157] = field::input(157, uint!(0_U256));
    signal_values[158] = field::input(158, uint!(0_U256));
    signal_values[159] = field::input(159, uint!(0_U256));
    signal_values[160] = field::input(160, uint!(0_U256));
    signal_values[161] = field::input(161, uint!(0_U256));
    signal_values[162] = field::input(162, uint!(0_U256));
    signal_values[163] = field::input(163, uint!(0_U256));
    signal_values[164] = field::input(164, uint!(0_U256));
    signal_values[165] = field::input(165, uint!(0_U256));
    signal_values[166] = field::input(166, uint!(0_U256));
    signal_values[167] = field::input(167, uint!(0_U256));
    signal_values[168] = field::input(168, uint!(1_U256));
    signal_values[169] = field::input(169, uint!(1_U256));
    signal_values[170] = field::input(170, uint!(0_U256));
}

/// Run cpp witness generator and optimize graph
pub fn build_witness() -> eyre::Result<()> {
    println!("get_total_signal_no: {}", ffi::get_total_signal_no());
    println!("get_main_input_signal_no: {}", ffi::get_main_input_signal_no());
    println!("get_main_input_signal_start: {}", ffi::get_main_input_signal_start());
    let mut signal_values = vec![field::undefined(); ffi::get_total_signal_no() as usize];
    signal_values[0] = field::constant(uint!(1_U256));

    let first_input_signal = ffi::get_main_input_signal_start() as usize;
    let total_input_len =
        (ffi::get_main_input_signal_no() + ffi::get_main_input_signal_start()) as usize;

    for i in first_input_signal..total_input_len {
        // println!("set signal #{}", i);
        signal_values[i] = field::input(i, uint!(0_U256));
    }
    // set_authV2_signals(&mut signal_values);

    let ioSignals = get_iosignals();
    let x = IOSignalsMap { m: ioSignals };
    let mut ctx = ffi::Circom_CalcWit {
        signalValues: signal_values,
        componentMemory: vec![
            ffi::Circom_Component::default();
            ffi::get_number_of_components() as usize
        ],
        circuitConstants: get_constants(),
        templateInsId2IOSignalInfoList: &x,
        listOfTemplateMessages: vec![],
    };

    // measure time
    let now = Instant::now();
    unsafe {
        ffi::run(&mut ctx as *mut _);
    }
    eprintln!("Calculation took: {:?}", now.elapsed());

    // Trace the calculation of a signal
    // {
    //     let nodes = field::get_graph();
    //     println!("signal value #1: {}, node: {:?}", ctx.signalValues[1].0, nodes[ctx.signalValues[1].0]);
    //     trace_signal(ctx.signalValues[1].0);
    // }

    let signal_values = get_witness_to_signal();
    let mut signals = signal_values
        .into_iter()
        .map(|i| ctx.signalValues[i].0)
        .collect::<Vec<_>>();

    // Print witness
    // {
    //     let vals = get_values();
    //     let mut j = 0;
    //     for i in signals {
    //         println!("wtns[#{}] = {}", j, vals[i]);
    //         j += 1;
    //     }
    // }

    let mut nodes = field::get_graph();
    eprintln!("Graph with {} nodes", nodes.len());

    // Optimize graph
    graph::optimize(&mut nodes, &mut signals);

    // Store graph to file.
    let input_map = get_input_hash_map();
    let bytes = postcard::to_stdvec(&(&nodes, &signals, &input_map)).unwrap();
    eprintln!("Graph size: {} bytes", bytes.len());
    std::fs::write("graph.bin", bytes).unwrap();

    // Evaluate the graph.
    let input_len = (ffi::get_main_input_signal_no() + ffi::get_main_input_signal_start()) as usize; // TODO: fetch from file
    let mut inputs = vec![U256::from(0); input_len];
    inputs[0] = U256::from(1);
    for i in 1..nodes.len() {
        if let Node::Input(j) = nodes[i] {
            inputs[j] = get_values()[i];
        } else {
            break;
        }
    }

    let now = Instant::now();
    for _ in 0..10 {
        _ = graph::evaluate(&nodes, &inputs, &signals);
    }
    eprintln!("Calculation took: {:?}", now.elapsed() / 10);

    // Print graph
    // for (i, node) in nodes.iter().enumerate() {
    //     println!("node[{}] = {:?}", i, node);
    // }
    // for (i, j) in signals.iter().enumerate() {
    //     println!("signal[{}] = node[{}]", i, j);
    // }

    Ok(())
}

pub unsafe fn get_element_index(a: *const FrElement) -> usize {
    return (*a).0;
}

fn get_signal_info(x: &IOSignalsMap, idx: u32) -> InputOutputList {
    x.m.get(&idx).unwrap().clone()
}

