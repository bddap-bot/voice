fn main() {
    let source = "shaders/avatar.wgsl";
    println!("cargo:rerun-if-changed={source}");
    let text = std::fs::read_to_string(source).unwrap();
    let module = naga::front::wgsl::parse_str(&text).unwrap_or_else(|error| panic!("{}", error.emit_to_string(&text)));
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::PUSH_CONSTANT).validate(&module).unwrap_or_else(|error| panic!("{error:?}"));
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let options = naga::back::spv::Options { flags: naga::back::spv::Options::default().flags - naga::back::spv::WriterFlags::ADJUST_COORDINATE_SPACE, ..Default::default() };
    for (stage, entry) in [(naga::ShaderStage::Vertex, "vertex"), (naga::ShaderStage::Fragment, "fragment")] {
        let pipeline = naga::back::spv::PipelineOptions { shader_stage: stage, entry_point: entry.into() };
        let words = naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline)).unwrap();
        std::fs::write(out.join(format!("{entry}.spv")), words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
    }
}
