#[cfg(not(test))]
#[path = "../main.rs"]
mod codewith_main;

#[cfg(not(test))]
fn main() -> anyhow::Result<()> {
    codewith_main::main()
}
