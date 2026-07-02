fn main() {
    for src in [
        "1", "1+2", "1+2*3", "10-4", "7%3", "2*3*4",
        "(1<2)", "(1<2)&&(3>=3)", "1<2?10:20", "-5", "!true",
        "1&3", "5|2", "6^3", "1<<4", "true", "false", "null", "undefined",
        "void 0", "1===1", "1!==2", "1.5+2.5",
    ] {
        match endor_oracle::run(src) {
            Some(o) => {
                let hex: Vec<String> = o.bytecode.iter().map(|b| format!("{:02x}", b)).collect();
                println!("SRC {:<16} ok={} result={:?} computrons={} nbytes={} sym={}",
                    src, o.completed, o.result, o.computrons, o.bytecode.len(), o.symbols.len());
                println!("    bytes: {}", hex.join(" "));
            }
            None => println!("SRC {:<16} <machine failure>", src),
        }
    }
}
