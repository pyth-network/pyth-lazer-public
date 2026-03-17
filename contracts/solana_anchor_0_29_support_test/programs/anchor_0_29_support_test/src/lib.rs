use anchor_lang::prelude::*;

declare_id!("7AJbUMU8e9i3DocCpCfvGFu3X12SAx5nPnAynd1nFaHD");

#[program]
pub mod anchor_0_29_support_test {
    use super::*;

    pub fn initialize(_ctx: Context<Initialize>) -> Result<()> {
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
