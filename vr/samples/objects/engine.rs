const DEGREE: f32 = PI / 180.0;
const HIDDEN: [f32; 3] = [0.0, 0.0, 0.0];

fn valve_lift(theta: f32, opens: f32) -> f32 {
    if theta >= opens && theta < opens + PI {
        let s = sin(theta - opens);
        scene::VALVE_LIFT * s * s
    } else {
        0.0
    }
}

fn puffs(first: usize, progress: f32, inward: bool, side: f32) {
    let inner = [side * scene::PORT_INNER_X, scene::PORT_INNER_Y, 0.0];
    let outer = [side * scene::PORT_OUTER_X, scene::PORT_OUTER_Y, 0.0];
    for k in 0..3 {
        let record = rest(first + k);
        if !(0.0..1.0).contains(&progress) {
            set_scale(record, HIDDEN);
            continue;
        }
        let f = fract(progress * 2.0 + k as f32 / 3.0);
        let along = if inward { 1.0 - f } else { f };
        let size = sin(PI * f) * (1.0 - progress * progress);
        set_translation(record, [inner[0] + (outer[0] - inner[0]) * along, inner[1] + (outer[1] - inner[1]) * along, 0.0]);
        set_scale(record, [size, size, size]);
    }
}

#[no_mangle]
pub extern "C" fn tick(time: f32, _x: f32, _y: f32, _z: f32, _touching: i32) -> i32 {
    let theta = time / scene::DURATION * 2.0 * TAU;
    let (r, l) = (scene::CRANK_RADIUS, scene::ROD_LENGTH);
    let pin = [r * sin(theta), r * cos(theta)];
    let wrist = pin[1] + sqrt(l * l - pin[0] * pin[0]);

    set_rotation(rest(scene::CRANKSHAFT), turn([0.0, 0.0, 1.0], -theta));
    place(rest(scene::CONNECTING_ROD), [pin[0], pin[1], 0.0], from_up([-pin[0] / l, (wrist - pin[1]) / l, 0.0]), [1.0, 1.0, 1.0]);
    set_translation(rest(scene::PISTON), [0.0, wrist - (r + l), 0.0]);

    let cam = turn([0.0, 0.0, 1.0], -theta / 2.0);
    set_rotation(rest(scene::INTAKE_CAM), cam);
    set_rotation(rest(scene::EXHAUST_CAM), cam);
    let intake = valve_lift(theta, 0.0);
    let exhaust = valve_lift(theta, 3.0 * PI);
    set_translation(rest(scene::INTAKE_VALVE), [intake * scene::VALVE_UX, -intake * scene::VALVE_UY, 0.0]);
    set_translation(rest(scene::EXHAUST_VALVE), [-exhaust * scene::VALVE_UX, -exhaust * scene::VALVE_UY, 0.0]);
    set_scale(rest(scene::INTAKE_SPRING), [1.0, 1.0 - intake / scene::SPRING_LENGTH, 1.0]);
    set_scale(rest(scene::EXHAUST_SPRING), [1.0, 1.0 - exhaust / scene::SPRING_LENGTH, 1.0]);

    let chamber = scene::DECK_Y - (wrist + scene::CROWN_ABOVE_PIN);
    let degrees = theta / DEGREE;
    let gases = [
        (scene::FRESH_CHARGE, degrees < 374.0, 1.0),
        (scene::COMBUSTION, (348.0..540.0).contains(&degrees), 0.05 + 0.95 * smoothstep(348.0, 374.0, degrees)),
        (scene::EXHAUST_GAS, degrees >= 540.0, 1.0),
    ];
    for (node, shown, radius) in gases {
        set_scale(rest(node), if shown { [radius, chamber, radius] } else { HIDDEN });
    }

    let spark = rest(scene::SPARK);
    if (330.0..352.0).contains(&degrees) {
        let flicker = 0.85 + 0.15 * sin(degrees * 2.3);
        set_rotation(spark, turn([0.0, 1.0, 0.0], degrees * 0.7));
        set_scale(spark, [flicker, flicker, flicker]);
    } else {
        set_scale(spark, HIDDEN);
    }

    puffs(scene::INTAKE_PUFF_0, theta / PI, true, -1.0);
    puffs(scene::EXHAUST_PUFF_0, (theta - 3.0 * PI) / PI, false, 1.0);
    publish()
}
