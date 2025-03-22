use anyhow::{anyhow, Result, Context};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    signature::{Keypair, Signer},
    pubkey::Pubkey,
    system_instruction,
    transaction::Transaction,
    signer::keypair::keypair_from_seed,
};
use std::sync::Arc;
use bip39::{Mnemonic, Language};

// simplified HD wallet manager
pub struct WalletManager {
    master_keypair: Keypair,
    derived_keypairs: Vec<Arc<Keypair>>,
}

impl WalletManager {
    // Create a new wallet manager from master keypair
    pub fn new(master_keypair: Keypair, count: usize) -> Result<Self> {
        let derived_keypairs = Self::derive_multiple_wallets(&master_keypair, count)?;
        
        Ok(Self {
            master_keypair,
            derived_keypairs,
        })
    }
    
    // create wallet manager from mnemonic phrase
    pub fn from_mnemonic(mnemonic_phrase: &str, count: usize) -> Result<Self> {
        // use BIP39 to convert mnemonic phrase to seed
        // fix method call
        let mnemonic = Mnemonic::parse_normalized(mnemonic_phrase)
            .map_err(|e| anyhow!("Invalid mnemonic phrase: {}", e))?;
            
        // use empty password to generate seed
        let seed = mnemonic.to_seed("");
        
        // create master keypair from seed
        let master_keypair = keypair_from_seed(&seed[..32])
            .map_err(|e| anyhow!("Failed to create keypair from seed: {}", e))?;
            
        Self::new(master_keypair, count)
    }
    
    // Derive multiple wallets using a simple deterministic approach
    fn derive_multiple_wallets(master_keypair: &Keypair, count: usize) -> Result<Vec<Arc<Keypair>>> {
        let mut derived_keypairs = Vec::with_capacity(count);
        
        // get seed material
        let seed_material = master_keypair.to_bytes();
        
        for i in 0..count {
            // create a specific seed for each index
            let derived_seed = derive_seed_at_index(&seed_material, i)?;
            
            // create keypair from derived seed
            let keypair = keypair_from_seed(&derived_seed)
                .map_err(|e| anyhow!("Failed to create derived keypair {}: {}", i, e))?;
                
            derived_keypairs.push(Arc::new(keypair));
        }
        
        Ok(derived_keypairs)
    }
    
    // Get master keypair
    pub fn master_keypair(&self) -> &Keypair {
        &self.master_keypair
    }
    
    // Get derived keypairs
    pub fn derived_keypairs(&self) -> &[Arc<Keypair>] {
        &self.derived_keypairs
    }
    
    // Fund derived wallets from master wallet
    pub async fn fund_derived_wallets(&self, client: &RpcClient, amount_per_wallet: u64) -> Result<()> {
        println!("Funding derived wallets...");
        
        for (i, keypair) in self.derived_keypairs.iter().enumerate() {
            let balance = client.get_balance(&keypair.pubkey())?;
            
            if balance < amount_per_wallet {
                let amount_to_transfer = amount_per_wallet - balance;
                
                println!("Funding wallet {} ({}) with {} SOL", 
                    i, 
                    keypair.pubkey().to_string(), 
                    amount_to_transfer as f64 / 1_000_000_000.0
                );
                
                let instruction = system_instruction::transfer(
                    &self.master_keypair.pubkey(),
                    &keypair.pubkey(),
                    amount_to_transfer,
                );
                
                let recent_blockhash = client.get_latest_blockhash()?;
                
                let transaction = Transaction::new_signed_with_payer(
                    &[instruction],
                    Some(&self.master_keypair.pubkey()),
                    &[&self.master_keypair],
                    recent_blockhash,
                );
                
                let signature = client.send_and_confirm_transaction(&transaction)?;
                println!("  Funded with transaction: {}", signature);
            } else {
                println!("Wallet {} already has sufficient balance: {} SOL", 
                    i, 
                    balance as f64 / 1_000_000_000.0
                );
            }
        }
        
        Ok(())
    }
    
    // Get total balance of all wallets
    pub fn get_total_balance(&self, client: &RpcClient) -> Result<u64> {
        let mut total = client.get_balance(&self.master_keypair.pubkey())?;
        
        for keypair in &self.derived_keypairs {
            total += client.get_balance(&keypair.pubkey())?;
        }
        
        Ok(total)
    }
}

// helper function to derive child seed from master seed and index - simplified version
fn derive_seed_at_index(master_seed: &[u8], index: usize) -> Result<[u8; 32]> {
    let mut derived_seed = [0u8; 32];
    
    // basic deterministic derivation method
    // copy part of master seed
    let copy_len = std::cmp::min(master_seed.len(), 20);
    derived_seed[..copy_len].copy_from_slice(&master_seed[..copy_len]);
    
    // mix index (encode index into 4 bytes)
    derived_seed[20] = (index & 0xFF) as u8;
    derived_seed[21] = ((index >> 8) & 0xFF) as u8;
    derived_seed[22] = ((index >> 16) & 0xFF) as u8;
    derived_seed[23] = ((index >> 24) & 0xFF) as u8;
    
    // mix more entropy
    for i in 24..32 {
        if i < master_seed.len() && (i - 24) < master_seed.len() {
            derived_seed[i] = master_seed[i] ^ master_seed[i - 24];
        } else if i < master_seed.len() {
            derived_seed[i] = master_seed[i];
        } else if (i - 24) < master_seed.len() {
            derived_seed[i] = master_seed[i - 24];
        } else {
            derived_seed[i] = i as u8; // last fallback
        }
    }
    
    Ok(derived_seed)
} 