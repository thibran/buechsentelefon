use vergen_gitcl::{Emitter, GitclBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Emitter::default()
        .add_instructions(&GitclBuilder::default().sha(true).build()?)?
        .emit()?;
    Ok(())
}
