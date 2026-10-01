//! Print the pointer-shape backend's answers at screen points, for
//! diagnosing presence cursor shapes inside a guest:
//!
//!   pointer_shape_at X,Y [X,Y ...]
//!
//! Hit-test only; never moves the pointer.
fn main() {
    #[cfg(target_os = "linux")]
    {
        if !platform_linux::install_pointer_shape_backend() {
            eprintln!("no pointer-shape backend (no display?)");
            std::process::exit(1);
        }
        let backend = cua_driver_core::pointer_shape::pointer_shape_backend().unwrap();
        println!(
            "names: {:?} limitation: {:?}",
            backend.names(),
            backend.limitation()
        );
        println!(
            "pointer: {:?} system: {:?}",
            backend.pointer_position(),
            backend.system_shape()
        );
        for arg in std::env::args().skip(1) {
            let Some((x, y)) = arg.split_once(',') else {
                continue;
            };
            let (x, y): (f64, f64) = (x.parse().unwrap(), y.parse().unwrap());
            let started = std::time::Instant::now();
            let hit = backend.hit_test(x, y);
            println!("{x},{y}: {hit:?} ({:?})", started.elapsed());
        }
    }
}
