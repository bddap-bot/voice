use wasmi::{Config, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TrapCode, TypedFunc};

/// The most linear memory a program may grow to.
pub const MEMORY: usize = 16 << 20;
/// Instructions a program may run per tick, and for its start function; running out stops the object.
pub const FUEL: u64 = 2_000_000;
/// Floats per node in what `tick` returns: translation xyz, rotation quaternion xyzw, scale xyz.
pub const STRIDE: usize = 10;

/// One node's local transform as a program sets it.
pub type Transform = [f32; STRIDE];

/// An object's WebAssembly program: no imports, a `memory` export, `tick(time, x, y, z, touching) -> pointer`, and optionally `data(length) -> pointer` for live data.
pub struct Program {
    store: Store<StoreLimits>,
    tick: TypedFunc<(f32, f32, f32, f32, i32), i32>,
    data: Option<TypedFunc<i32, i32>>,
    memory: Memory,
    nodes: usize,
}

fn refused(error: wasmi::Error) -> String {
    match error.as_trap_code() {
        Some(TrapCode::OutOfFuel) => format!("ran past its {FUEL}-instruction budget"),
        _ => format!("program: {error}"),
    }
}

impl Program {
    pub fn new(wasm: &[u8], nodes: usize) -> Result<Program, String> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, wasm).map_err(|error| format!("program: {error}"))?;
        if let Some(import) = module.imports().next() {
            return Err(format!("program imports {}.{}; a program may import nothing", import.module(), import.name()));
        }
        let mut store = Store::new(&engine, StoreLimitsBuilder::new().memory_size(MEMORY).memories(1).tables(1).table_elements(1 << 16).instances(1).build());
        store.limiter(|limits| limits);
        store.set_fuel(FUEL).map_err(refused)?;
        let instance = Linker::new(&engine).instantiate_and_start(&mut store, &module).map_err(refused)?;
        let memory = instance.get_memory(&store, "memory").ok_or("program exports no memory")?;
        let tick = instance.get_typed_func(&store, "tick").map_err(|error| format!("program tick: {error}"))?;
        let data = match instance.get_export(&store, "data") {
            Some(_) => Some(instance.get_typed_func(&store, "data").map_err(|error| format!("program data: {error}"))?),
            None => None,
        };
        Ok(Program { store, tick, data, memory, nodes })
    }

    /// Hands the program new live data: `data(length)` names where to write it, and the next tick sees it. A program without `data` ignores it.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
        let Some(data) = &self.data else { return Ok(()) };
        self.store.set_fuel(FUEL).map_err(refused)?;
        let length = i32::try_from(bytes.len()).map_err(|_| "live data too long")?;
        let at = data.call(&mut self.store, length).map_err(refused)?;
        if at == 0 {
            return Ok(());
        }
        self.memory.write(&mut self.store, at as u32 as usize, bytes).map_err(|_| format!("data returned {at}, outside its memory"))
    }

    /// Runs one tick; `tip` is the touch point in the object's own coordinates. `None` keeps the last transforms.
    pub fn tick(&mut self, time: f32, tip: Option<[f32; 3]>) -> Result<Option<Vec<Transform>>, String> {
        self.store.set_fuel(FUEL).map_err(refused)?;
        let [x, y, z] = tip.unwrap_or_default();
        let at = self.tick.call(&mut self.store, (time, x, y, z, tip.is_some() as i32)).map_err(refused)?;
        if at == 0 {
            return Ok(None);
        }
        let mut bytes = vec![0; self.nodes * STRIDE * 4];
        self.memory.read(&self.store, at as u32 as usize, &mut bytes).map_err(|_| format!("tick returned {at}, outside its memory"))?;
        let floats: Vec<f32> = bytes.chunks_exact(4).map(|word| f32::from_le_bytes(word.try_into().unwrap())).collect();
        if floats.iter().any(|value| !value.is_finite()) {
            return Err("tick returned a transform that is not finite".into());
        }
        Ok(Some(floats.chunks_exact(STRIDE).map(|transform| transform.try_into().unwrap()).collect()))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A program for `nodes` nodes that moves node 0 to x = time and node 1 to the touch point.
    pub const FOLLOW: &str = r#"(module
        (memory (export "memory") 1)
        (func $one (param $at i32) (f32.store (local.get $at) (f32.const 1)))
        (func (export "tick") (param $time f32) (param $x f32) (param $y f32) (param $z f32) (param $touching i32) (result i32)
            (f32.store (i32.const 1024) (local.get $time))
            (call $one (i32.const 1048)) (call $one (i32.const 1052)) (call $one (i32.const 1056)) (call $one (i32.const 1060))
            (f32.store (i32.const 1064) (local.get $x)) (f32.store (i32.const 1068) (local.get $y)) (f32.store (i32.const 1072) (local.get $z))
            (call $one (i32.const 1088)) (call $one (i32.const 1092)) (call $one (i32.const 1096)) (call $one (i32.const 1100))
            (i32.const 1024)))"#;

    pub fn wasm(text: &str) -> Vec<u8> {
        wat::parse_str(text).unwrap()
    }

    #[test]
    fn a_program_sets_node_transforms_from_time_and_the_touch_point() {
        let mut program = Program::new(&wasm(FOLLOW), 2).unwrap();
        let transforms = program.tick(2.5, Some([0.1, 0.2, 0.3])).unwrap().unwrap();
        assert_eq!(transforms, [[2.5, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0], [0.1, 0.2, 0.3, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]]);
    }

    /// Keeps the first byte of its last data in node 0's x.
    pub const GAUGE: &str = r#"(module
        (memory (export "memory") 1)
        (func (export "data") (param $length i32) (result i32) (i32.const 4096))
        (func (export "tick") (param f32 f32 f32 f32 i32) (result i32)
            (f32.store (i32.const 1024) (f32.convert_i32_u (i32.load8_u (i32.const 4096))))
            (f32.store (i32.const 1048) (f32.const 1))
            (f32.store (i32.const 1052) (f32.const 1)) (f32.store (i32.const 1056) (f32.const 1)) (f32.store (i32.const 1060) (f32.const 1))
            (i32.const 1024)))"#;

    #[test]
    fn live_data_reaches_the_program_through_its_data_export() {
        let mut program = Program::new(&wasm(GAUGE), 1).unwrap();
        program.feed(b"*rest").unwrap();
        assert_eq!(program.tick(0.0, None).unwrap().unwrap()[0][0], 42.0);
        program.feed(&[7]).unwrap();
        assert_eq!(program.tick(0.0, None).unwrap().unwrap()[0][0], 7.0);
        let outside = r#"(module (memory (export "memory") 1) (func (export "data") (param i32) (result i32) (i32.const 65535)) (func (export "tick") (param f32 f32 f32 f32 i32) (result i32) (i32.const 0)))"#;
        assert!(Program::new(&wasm(outside), 1).unwrap().feed(b"too long").err().unwrap().contains("outside its memory"));
        let quiet = r#"(module (memory (export "memory") 1) (func (export "tick") (param f32 f32 f32 f32 i32) (result i32) (i32.const 0)))"#;
        assert_eq!(Program::new(&wasm(quiet), 1).unwrap().feed(b"ignored"), Ok(()), "a program without data ignores it");
    }

    #[test]
    fn a_program_that_imports_loops_overgrows_or_points_outside_its_memory_is_refused() {
        let imports = r#"(module (import "env" "clock" (func)) (memory (export "memory") 1) (func (export "tick") (param f32 f32 f32 f32 i32) (result i32) (i32.const 0)))"#;
        assert!(Program::new(&wasm(imports), 1).err().unwrap().contains("may import nothing"));
        let spin = r#"(module (memory (export "memory") 1) (func (export "tick") (param f32 f32 f32 f32 i32) (result i32) (loop $l (br $l)) (i32.const 0)))"#;
        let mut program = Program::new(&wasm(spin), 1).unwrap();
        assert!(program.tick(0.0, None).err().unwrap().contains("budget"));
        let start = r#"(module (memory (export "memory") 1) (func $s (loop $l (br $l))) (start $s) (func (export "tick") (param f32 f32 f32 f32 i32) (result i32) (i32.const 0)))"#;
        assert!(Program::new(&wasm(start), 1).err().unwrap().contains("budget"));
        let greedy = format!(r#"(module (memory (export "memory") 1) (func (export "tick") (param f32 f32 f32 f32 i32) (result i32) (if (i32.lt_s (memory.grow (i32.const {})) (i32.const 0)) (then unreachable)) (i32.const 0)))"#, MEMORY / 65536);
        let mut program = Program::new(&wasm(&greedy), 1).unwrap();
        assert!(program.tick(0.0, None).is_err(), "growing past {MEMORY} bytes fails");
        let outside = r#"(module (memory (export "memory") 1) (func (export "tick") (param f32 f32 f32 f32 i32) (result i32) (i32.const 65530)))"#;
        assert!(Program::new(&wasm(outside), 1).unwrap().tick(0.0, None).err().unwrap().contains("outside its memory"));
        let quiet = r#"(module (memory (export "memory") 1) (func (export "tick") (param f32 f32 f32 f32 i32) (result i32) (i32.const 0)))"#;
        assert_eq!(Program::new(&wasm(quiet), 1).unwrap().tick(0.0, None).unwrap(), None, "zero keeps the last transforms");
    }
}
