use std::path::Path;

use disk::partition::auto_create_partitions_gpt;

fn main() {
    auto_create_partitions_gpt(Path::new("/dev/loop30")).unwrap();
}
