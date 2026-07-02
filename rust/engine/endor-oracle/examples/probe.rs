fn main() {
    let progs: Vec<String> = std::env::args().skip(1).collect();
    let progs = if progs.is_empty() {
        vec![
            "(function(x){return x+1})(5)".to_string(),
            "(function(){return 3})()".to_string(),
            "(x=>x*2)(21)".to_string(),
            "((a,b)=>a+b)(2,3)".to_string(),
        ]
    } else { progs };
    for src in progs {
        match endor_oracle::run(&src) {
            Some(o) => {
                let dis = endor_vm::disassemble(&o.bytecode);
                let names: Vec<String> = dis.iter().map(|(_,n)| n.to_string()).collect();
                println!("SRC {:<32} ok={} result={:?} comp={} nbytes={}",
                    src, o.completed, o.result, o.computrons, o.bytecode.len());
                println!("    dis: {}", names.join(" "));
                let hex: Vec<String> = o.bytecode.iter().map(|b| format!("{:02x}", b)).collect();
                println!("    hex: {}", hex.join(" "));
            }
            None => println!("SRC {} <fail>", src),
        }
    }
}
