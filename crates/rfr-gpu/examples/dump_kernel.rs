//! Prints the layer kernel (WGSL) of the first layer of a document in the mode its view needs:
//! `dump_kernel <file.rfr>` (shader work: read what the hot loop runs).
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let doc = rfr_core::io::open(std::path::Path::new(&args[1])).unwrap();
    let plan = rfr_core::render::Plan::sized(&doc, 64, 36).unwrap();
    let lp = &plan.layers()[0];
    let mode = rfr_gpu::renderer::layer_mode(&plan, lp);
    eprintln!("mode {mode:?}");
    print!("{}", rfr_gpu::codegen::kernel(lp, mode));
}
