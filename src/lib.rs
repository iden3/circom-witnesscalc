#[allow(non_snake_case,dead_code)]
mod field;
pub mod graph;

use ruint::aliases::U256;


/// Allocates inputs vec with position 0 set to 1
pub fn get_inputs_buffer(size: usize) -> Vec<U256> {
    let mut inputs = vec![U256::ZERO; size];
    inputs[0] = U256::from(1);
    inputs
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_ok() {
        // build_witness();
        for i in 0..3 {
            println!("{}", i);
        }
        println!("OK");
    }

}