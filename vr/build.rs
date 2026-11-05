fn main() {
    let source = "shaders/avatar.wgsl";
    println!("cargo:rerun-if-changed={source}");
    let text = std::fs::read_to_string(source).unwrap();
    let module = naga::front::wgsl::parse_str(&text).unwrap_or_else(|error| panic!("{}", error.emit_to_string(&text)));
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::PUSH_CONSTANT).validate(&module).unwrap_or_else(|error| panic!("{error:?}"));
    let pages = module.global_variables.iter().find(|(_, global)| global.name.as_deref() == Some("decal_pages")).map(|(_, global)| match module.types[global.ty].inner {
        naga::TypeInner::BindingArray { size: naga::ArraySize::Constant(size), .. } => size.get(),
        _ => panic!("decal_pages is not a binding array of a fixed size"),
    }).expect("the shader has no decal_pages");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let options = naga::back::spv::Options { flags: naga::back::spv::Options::default().flags - naga::back::spv::WriterFlags::ADJUST_COORDINATE_SPACE, ..Default::default() };
    let write = |stage, entry: &str, used: u32, file: String| {
        let constants = naga::back::PipelineConstants::from_iter([("decal_pages_used".to_owned(), used as f64)]);
        let (module, info) = naga::back::pipeline_constants::process_overrides(&module, &info, Some((stage, entry)), &constants).unwrap_or_else(|error| panic!("{error:?}"));
        let pipeline = naga::back::spv::PipelineOptions { shader_stage: stage, entry_point: entry.into() };
        let words = naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline)).unwrap();
        std::fs::write(out.join(file), words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
    };
    write(naga::ShaderStage::Vertex, "vertex", 0, "vertex.spv".into());
    write(naga::ShaderStage::Fragment, "depth", 0, "depth.spv".into());
    let mut shaders = format!("const DECAL_PAGES: usize = {pages};\n");
    for (entry, name) in [("fragment", "FRAGMENT_SHADERS"), ("cutout", "CUTOUT_SHADERS")] {
        for used in 0..=pages {
            write(naga::ShaderStage::Fragment, entry, used, format!("{entry}-{used}.spv"));
        }
        let modules: String = (0..=pages).map(|used| format!("include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{entry}-{used}.spv\")),")).collect();
        shaders += &format!("const {name}: [&[u8]; {}] = [{modules}];\n", pages + 1);
    }
    std::fs::write(out.join("shaders.rs"), shaders).unwrap();
}
