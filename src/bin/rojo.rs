//! The `rojo` alias binary. Roxo is a drop-in replacement for Rojo, so it
//! installs under both names and behaves identically under either.

fn main() {
    libroxo::entrypoint::main();
}
