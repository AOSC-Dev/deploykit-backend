use std::path::Path;

use disk::partition::auto_create_partitions_gptman;

fn main() {
    auto_create_partitions_gptman(Path::new("/dev/loop30")).unwrap();
    
}