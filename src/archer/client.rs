use anyhow::{Context, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use super::accounts;
use super::config::MarketConfig;
use super::types::MakerBook;

pub struct ArcherClient {
    rpc: RpcClient,
}

#[derive(Debug, Clone)]
pub struct SendOptions {
    pub priority_fee_micro_lamports: Option<u64>,
    pub compute_unit_limit: Option<u32>,
    pub max_retries: u32,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            priority_fee_micro_lamports: None,
            compute_unit_limit: None,
            max_retries: 3,
        }
    }
}

impl SendOptions {
    pub fn with_priority_fee(mut self, micro_lamports: u64) -> Self {
        self.priority_fee_micro_lamports = Some(micro_lamports);
        self
    }
}

impl ArcherClient {
    pub fn new(rpc_url: &str) -> Self {
        Self {
            rpc: RpcClient::new_with_commitment(
                rpc_url.to_string(),
                CommitmentConfig::confirmed(),
            ),
        }
    }

    pub async fn get_market_config(&self, market: &Pubkey) -> Result<MarketConfig> {
        let account = self
            .rpc
            .get_account(market)
            .await
            .context("Failed to fetch market account")?;

        let header = accounts::parse_market_state(&account.data)?;

        let base_mint_account = self
            .rpc
            .get_account(&header.base_mint)
            .await
            .context("Failed to fetch base mint")?;
        let quote_mint_account = self
            .rpc
            .get_account(&header.quote_mint)
            .await
            .context("Failed to fetch quote mint")?;

        let base_decimals = base_mint_account.data[44];
        let quote_decimals = quote_mint_account.data[44];

        Ok(MarketConfig::from_header(
            *market,
            header,
            base_decimals,
            quote_decimals,
            base_mint_account.owner,
            quote_mint_account.owner,
        ))
    }

    pub async fn get_maker_book(&self, market: &Pubkey, maker: &Pubkey) -> Result<MakerBook> {
        let (pda, _) = MakerBook::get_address(market, maker);
        let account = self
            .rpc
            .get_account(&pda)
            .await
            .context("Failed to fetch maker book account")?;
        let book = MakerBook::load(&account.data)?;
        Ok(*book)
    }

    pub async fn send_instructions(
        &self,
        instructions: &[Instruction],
        signers: &[&Keypair],
        options: SendOptions,
    ) -> Result<Signature> {
        let mut all_ixs = Vec::with_capacity(instructions.len() + 2);

        if let Some(limit) = options.compute_unit_limit {
            all_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(limit));
        }
        if let Some(fee) = options.priority_fee_micro_lamports {
            all_ixs.push(ComputeBudgetInstruction::set_compute_unit_price(fee));
        }

        all_ixs.extend_from_slice(instructions);

        let blockhash = self
            .rpc
            .get_latest_blockhash()
            .await
            .context("Failed to get blockhash")?;

        let payer = signers[0].pubkey();
        let tx = Transaction::new_signed_with_payer(&all_ixs, Some(&payer), signers, blockhash);

        let mut last_err = None;
        for _ in 0..=options.max_retries {
            match self.rpc.send_and_confirm_transaction(&tx).await {
                Ok(sig) => return Ok(sig),
                Err(e) => last_err = Some(e),
            }
        }

        Err(last_err.unwrap().into())
    }
}
