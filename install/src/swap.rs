pub fn get_recommend_swap_size(mem: u64) -> f64 {
    // 1073741824 is 1 * 1024 * 1024 * 1024 (1GiB => 1iB)
    let swap_size = match mem {
        x @ ..=1073741824 => (x * 2) as f64,
        x @ 1073741825.. => {
            let x = x as f64;
            x + x.sqrt().round()
        }
    };

    swap_size
}

