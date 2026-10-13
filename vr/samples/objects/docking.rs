/// Caffeine drifts in from the extracellular side along a wavering path, tumbling, and settles in its crystal pose at `APPROACH`; its interactions grow in as it binds.
#[no_mangle]
pub extern "C" fn tick(time: f32, _x: f32, _y: f32, _z: f32, _touching: i32) -> i32 {
    let away = 1.0 - smoothstep(0.0, scene::APPROACH, time);
    let sway = 0.25 * away * sin(3.0 * core::f32::consts::PI * away);
    let ligand = rest(scene::CAFFEINE);
    let at = [ligand[0] + scene::START_X * away + scene::START_Z * sway, ligand[1] + scene::START_Y * away, ligand[2] + scene::START_Z * away - scene::START_X * sway];
    set_translation(ligand, at);
    set_rotation(ligand, turn([0.48, 0.6, 0.64], 3.2 * core::f32::consts::PI * away));
    let grown = smoothstep(scene::GROW_FROM, scene::GROW_TO, time).max(0.001);
    for node in [scene::HBOND_ASN253, scene::PI_STACK_PHE168] {
        set_scale(rest(node), [grown; 3]);
    }
    publish()
}
