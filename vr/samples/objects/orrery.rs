/// Years since J2000 at the start of the timeline: the build's date until live data names another.
static mut EPOCH: f32 = scene::BUILT_YEARS;

/// Live data is a date, `2026-10-12` or `{"date":"1969-07-20"}`.
#[no_mangle]
pub extern "C" fn data(length: i32) -> i32 {
    receive(length)
}

fn years_since_j2000(text: &[u8]) -> Option<f32> {
    let year = number(text, b"")?;
    let after_year = text.iter().position(|b| b.is_ascii_digit())? + 4;
    let month = number(&text[after_year + 1..], b"").unwrap_or(1.0);
    let day = text.get(after_year + 1..).and_then(|rest| rest.iter().position(|b| *b == b'-').map(|at| &rest[at + 1..])).and_then(|rest| number(rest, b"")).unwrap_or(1.0);
    const BEFORE: [f32; 12] = [0.0, 31.0, 59.0, 90.0, 120.0, 151.0, 181.0, 212.0, 243.0, 273.0, 304.0, 334.0];
    let leap = (year as i32 % 4 == 0 && month > 2.0) as i32 as f32;
    let day_of_year = BEFORE[clamp(month - 1.0, 0.0, 11.0) as usize] + leap + day - 1.0;
    Some((year - 2000.0) + (day_of_year - 0.5) / 365.25)
}

#[no_mangle]
pub extern "C" fn tick(time: f32, _x: f32, _y: f32, _z: f32, _touching: i32) -> i32 {
    let (text, fresh) = inbox();
    if fresh {
        if let Some(years) = years_since_j2000(text) {
            unsafe { EPOCH = years };
        }
    }
    let years = unsafe { EPOCH } + time / scene::SECONDS_PER_YEAR;
    let planets = [
        (scene::MERCURY, scene::MERCURY_AU, scene::MERCURY_E, scene::MERCURY_PERIHELION, scene::MERCURY_J2000, scene::MERCURY_PERIOD),
        (scene::VENUS, scene::VENUS_AU, scene::VENUS_E, scene::VENUS_PERIHELION, scene::VENUS_J2000, scene::VENUS_PERIOD),
        (scene::EARTH, scene::EARTH_AU, scene::EARTH_E, scene::EARTH_PERIHELION, scene::EARTH_J2000, scene::EARTH_PERIOD),
        (scene::MARS, scene::MARS_AU, scene::MARS_E, scene::MARS_PERIHELION, scene::MARS_J2000, scene::MARS_PERIOD),
        (scene::JUPITER, scene::JUPITER_AU, scene::JUPITER_E, scene::JUPITER_PERIHELION, scene::JUPITER_J2000, scene::JUPITER_PERIOD),
        (scene::SATURN, scene::SATURN_AU, scene::SATURN_E, scene::SATURN_PERIHELION, scene::SATURN_J2000, scene::SATURN_PERIOD),
    ];
    for (node, a, e, perihelion, longitude, period) in planets {
        let mean = longitude + years / period * core::f32::consts::TAU - perihelion;
        let true_anomaly = mean + (2.0 * e - e * e * e / 4.0) * sin(mean) + 1.25 * e * e * sin(2.0 * mean) + 13.0 / 12.0 * e * e * e * sin(3.0 * mean);
        let r = a * (1.0 - e * e) / (1.0 + e * cos(true_anomaly));
        let drawn = scene::OUTER * sqrt(r / scene::OUTER_AU);
        let angle = true_anomaly + perihelion;
        set_translation(rest(node), [drawn * cos(angle), 0.0, -drawn * sin(angle)]);
    }
    let moon = scene::MOON_J2000 + years / scene::MOON_PERIOD * core::f32::consts::TAU;
    set_translation(rest(scene::MOON), [scene::MOON_DISTANCE * cos(moon), 0.0, -scene::MOON_DISTANCE * sin(moon)]);
    set_rotation(rest(scene::SUN), turn([0.0, 1.0, 0.0], years / (25.4 / 365.25) * core::f32::consts::TAU));
    publish()
}
