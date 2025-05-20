use anyhow::Result;

pub mod helpers;
pub mod svm_to_svm_e2e;
pub mod utils;

pub fn main() -> Result<()> {
    svm_to_svm_e2e::svm_to_svm_e2e()?;
    Ok(())
}
