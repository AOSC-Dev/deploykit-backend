use std::path::Path;

use disk::partition::auto_create_partitions_mbr;

fn main() {
    auto_create_partitions_mbr(Path::new("/dev/loop30")).unwrap();
}
