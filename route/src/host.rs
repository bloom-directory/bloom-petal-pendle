use crate::{Chain, Result};
use petal::sdk::{EvmTransaction, HttpRequest, HttpResponse, OutboxInspection, StagedTransaction};

pub trait Host {
    fn now_ms(&mut self) -> u64;
    fn http(&mut self, request: HttpRequest, max: usize) -> Result<HttpResponse>;
    fn vfs(&mut self, path: &str) -> Result<Vec<u8>>;
    fn chain(&mut self, chain: Chain, method: &str, params: &str) -> Result<String>;
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>>;
    fn put(&mut self, key: &str, value: &[u8]) -> Result<()>;
    fn put_new(&mut self, key: &str, value: &[u8]) -> Result<()>;
    fn delete_if(&mut self, key: &str, value: &[u8]) -> Result<()>;
    fn stage(&mut self, tx: &EvmTransaction) -> Result<StagedTransaction>;
    fn confirm(&mut self, wallet: &str, chain: Chain, id: &str) -> Result<StagedTransaction>;
    fn inspect(&mut self, wallet: &str, chain: Chain, id: &str) -> Result<OutboxInspection>;
}
pub struct BloomHost;
impl Host for BloomHost {
    fn now_ms(&mut self) -> u64 {
        petal::sdk::now_ms()
    }
    fn http(&mut self, req: HttpRequest, max: usize) -> Result<HttpResponse> {
        petal::sdk::http_fetch(&req, max).map_err(|_| "Pendle HTTP transport failed".into())
    }
    fn vfs(&mut self, path: &str) -> Result<Vec<u8>> {
        petal::sdk::vfs_read(path, 128)
            .map_err(|_| "selected account address is unavailable".into())
    }
    fn chain(&mut self, chain: Chain, method: &str, params: &str) -> Result<String> {
        petal::sdk::chain_read(chain.name(), method, params)
            .map_err(|_| format!("{} chain read failed", chain.name()))
    }
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>> {
        match petal::sdk::store_get(key, 128 * 1024) {
            Ok(v) => Ok(Some(v)),
            Err(petal::SdkError::Host(petal::HostStatus::NotFound)) => Ok(None),
            Err(_) => Err("operation store read failed".into()),
        }
    }
    fn put(&mut self, key: &str, value: &[u8]) -> Result<()> {
        petal::sdk::store_put(key, value, false).map_err(|_| "operation store write failed".into())
    }
    fn put_new(&mut self, key: &str, value: &[u8]) -> Result<()> {
        petal::sdk::store_put_new(key, value, false).map_err(|_| {
            "operation already exists or is locked; inspect status before retrying".into()
        })
    }
    fn delete_if(&mut self, key: &str, value: &[u8]) -> Result<()> {
        petal::sdk::store_del_if_value(key, value)
            .map_err(|_| "operation lock could not be released".into())
    }
    fn stage(&mut self, tx: &EvmTransaction) -> Result<StagedTransaction> {
        petal::sdk::tx_stage(tx).map_err(|_| {
            "outbox stage did not return an id; inspect the account outbox before recovery".into()
        })
    }
    fn confirm(&mut self, wallet: &str, chain: Chain, id: &str) -> Result<StagedTransaction> {
        petal::sdk::tx_confirm(wallet, chain.name(), id, false).map_err(|_| {
            "Bloom has not confirmed the transaction; inspect its owner approval and outbox".into()
        })
    }
    fn inspect(&mut self, wallet: &str, chain: Chain, id: &str) -> Result<OutboxInspection> {
        petal::sdk::tx_inspect(wallet, chain.name(), id)
            .map_err(|_| "outbox inspection failed".into())
    }
}

pub fn account_address<H: Host>(host: &mut H, account: &petal::WalletAccount) -> Result<String> {
    petal::parse_evm_address(&host.vfs(&account.address_evm_path())?)
}
