use std::{env, fs, process};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: cove-layout-inspect <layout-plan-split-or-zero-copy-section.bin>");
        process::exit(2);
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{path}: {error}");
            process::exit(1);
        }
    };
    if let Ok(plan) = cove_layout::LayoutPlanV2::parse(&bytes) {
        println!(
            "valid COVE-L layout plan: layout_id={} nodes={} root={}",
            plan.header.layout_id,
            plan.nodes.len(),
            plan.header.root_node_id
        );
        return;
    }
    if let Ok(index) = cove_layout::ScanSplitIndexV2::parse(&bytes) {
        println!(
            "valid COVE-L scan split index: splits={}",
            index.entries.len()
        );
        return;
    }
    match cove_layout::ZeroCopyBufferMapV2::parse(&bytes) {
        Ok(map) => {
            println!(
                "valid COVE-L zero-copy buffer map: targets={} entries={}",
                map.targets.len(),
                map.entries.len()
            );
        }
        Err(error) => {
            eprintln!(
                "{path}: not a valid COVE-L layout plan, scan split index, or zero-copy map: {error}"
            );
            process::exit(1);
        }
    }
}
