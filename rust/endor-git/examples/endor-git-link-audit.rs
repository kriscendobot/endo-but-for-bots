fn main() {
    let (major, minor, revision) = endor_git::libgit2_version();
    println!("libgit2 {major}.{minor}.{revision} (vendored, local-storage profile)");
}
