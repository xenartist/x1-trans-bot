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
use ed25519_dalek::SigningKey;
use hmac::{Hmac, Mac};
use sha2::Sha512;

type HmacSha512 = Hmac<Sha512>;

// simplified HD wallet manager
pub struct WalletManager {
    master_keypair: Keypair,     // master keypair generated directly from seed
    derived_keypairs: Vec<Keypair>, // derived sub wallets through BIP44 standard
    mnemonic: Option<String>,    // save mnemonic for use when needed
}

impl WalletManager {
    // create wallet manager, use provided master_keypair and seed passphrase
    // master_keypair is generated from seed in config, used for funding
    // derived wallets are derived from seed using BIP44 standard
    pub fn new(master_keypair: Keypair, mnemonic_phrase: Option<String>, num_wallets: usize) -> Result<Self> {
        if num_wallets == 0 {
            return Err(anyhow!("Number of wallets must be greater than 0"));
        }
        
        let master_pubkey = master_keypair.pubkey();
        println!("Using master wallet for funding: {}", master_pubkey);
        
        // derive sub wallets - use BIP44 standard if there is a seed passphrase, otherwise use simple derivation
        let derived_keypairs = if let Some(phrase) = &mnemonic_phrase {
            Self::derive_hd_wallets_from_mnemonic(phrase, num_wallets)?
        } else {
            // if there is no seed passphrase, fallback to simple derivation
            let mut keypairs = Vec::with_capacity(num_wallets);
            for i in 0..num_wallets {
                let derived = Self::derive_keypair_from_master(&master_keypair, i)?;
                println!("Derived wallet {} with pubkey: {} (simple derivation)", 
                    i, derived.pubkey());
                keypairs.push(derived);
            }
            keypairs
        };
        
        println!("Created {} derived wallets", derived_keypairs.len());
        
        Ok(Self {
            master_keypair,
            derived_keypairs,
            mnemonic: mnemonic_phrase,
        })
    }
    
    // derive sub wallets from seed using BIP44 standard
    fn derive_hd_wallets_from_mnemonic(phrase: &str, num_wallets: usize) -> Result<Vec<Keypair>> {
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, phrase)
            .context("Invalid mnemonic phrase")?;
        
        // generate seed
        let seed = mnemonic.to_seed("");
        
        println!("Deriving HD wallets using BIP44 standard path m/44'/501'/0'/i");
        
        let mut derived_keypairs = Vec::with_capacity(num_wallets);
        
        for i in 0..num_wallets {
            // derive path m/44'/501'/0'/i
            let wallet_path = format!("m/44'/501'/0'/{}", i);
            let (child_keypair, _) = Self::derive_path_from_seed(&seed, &wallet_path)
                .context(format!("Failed to derive key for path {}", wallet_path))?;
            
            println!("Derived wallet {} with pubkey: {} (path: {})", 
                i, child_keypair.pubkey(), wallet_path);
            
            derived_keypairs.push(child_keypair);
        }
        
        Ok(derived_keypairs)
    }
    
    // Get master keypair
    pub fn master_keypair(&self) -> &Keypair {
        &self.master_keypair
    }
    
    // Get derived keypairs
    pub fn derived_keypairs(&self) -> &[Keypair] {
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

    // get derived keypair from master keypair
    fn derive_keypair_from_master(master: &Keypair, index: usize) -> Result<Keypair> {
        // use HMAC-SHA512 to derive sub wallet key
        let mut mac = HmacSha512::new_from_slice(master.pubkey().as_ref())
            .context("Failed to create HMAC")?;
        
        // use index as message
        mac.update(&index.to_le_bytes());
        
        // get HMAC result
        let result = mac.finalize().into_bytes();
        
        // use first 32 bytes of HMAC result as new private key
        let keypair = Keypair::from_bytes(&result[0..32])
            .context("Failed to create keypair from derived bytes")?;
        
        Ok(keypair)
    }

    // parse BIP32/BIP44 path
    fn parse_path(path: &str) -> Result<Vec<u32>> {
        let mut result = Vec::new();
        
        // remove leading "m/"
        let path = path.strip_prefix("m/").unwrap_or(path);
        
        // parse path segments
        for segment in path.split('/') {
            if segment.is_empty() {
                continue;
            }
            
            // check if it is a hardened derivation (with ' suffix)
            let hardened = segment.ends_with('\'');
            let index_str = if hardened {
                &segment[..segment.len() - 1]
            } else {
                segment
            };
            
            // parse index
            let index = index_str.parse::<u32>()
                .context(format!("Invalid path segment: {}", segment))?;
            
            // if it is a hardened derivation, add hardened flag (0x80000000)
            let final_index = if hardened {
                index | 0x80000000
            } else {
                index
            };
            
            result.push(final_index);
        }
        
        Ok(result)
    }

    // derive keypair from seed for specific path
    fn derive_path_from_seed(seed: &[u8], path: &str) -> Result<(Keypair, [u8; 32])> {
        // parse HD path
        let indices = Self::parse_path(path)?;
        
        // generate master key
        let mut hmac = HmacSha512::new_from_slice(b"ed25519 seed")
            .context("Failed to create HMAC for master key")?;
        hmac.update(seed);
        let i = hmac.finalize().into_bytes();
        
        let mut key = [0u8; 32];
        let mut chain_code = [0u8; 32];
        
        key.copy_from_slice(&i[0..32]);
        chain_code.copy_from_slice(&i[32..64]);
        
        // derive sub key
        for child_index in indices {
            // prepare data for derivation
            let mut data = Vec::with_capacity(37);
            
            if child_index & 0x80000000 != 0 {
                // Hardened key: 0x00 || parent_key || index
                data.push(0);
                data.extend_from_slice(&key);
            } else {
                // Normal key: public_key || index
                // calculate public key from private key - use ed25519-dalek 2.x API
                let signing_key = SigningKey::from_bytes(&key);
                let verifying_key = signing_key.verifying_key();
                
                data.extend_from_slice(&verifying_key.to_bytes());
            }
            
            // add index (big endian)
            data.extend_from_slice(&child_index.to_be_bytes());
            
            // calculate sub key
            let mut hmac = HmacSha512::new_from_slice(&chain_code)
                .context("Failed to create HMAC for child key")?;
            hmac.update(&data);
            let i = hmac.finalize().into_bytes();
            
            key.copy_from_slice(&i[0..32]);
            chain_code.copy_from_slice(&i[32..64]);
        }
        
        // calculate keypair using ed25519-dalek 2.x
        let signing_key = SigningKey::from_bytes(&key);
        let verifying_key = signing_key.verifying_key();
        
        // convert to Solana format Keypair
        let mut keypair_bytes = [0u8; 64];
        keypair_bytes[0..32].copy_from_slice(&key);
        keypair_bytes[32..64].copy_from_slice(&verifying_key.to_bytes());
        
        let solana_keypair = Keypair::from_bytes(&keypair_bytes)
            .context("Failed to create Solana keypair")?;
        
        Ok((solana_keypair, chain_code))
    }
} 