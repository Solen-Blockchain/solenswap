//! SolenDEX — AMM (Automated Market Maker) for STT/SOLEN trading
//!
//! Uniswap V2-style constant product AMM (x * y = k).
//!
//! ## How it works
//!
//! The DEX maintains a liquidity pool with reserves of both SOLEN (native) and
//! STT (SRC-20 token). Liquidity providers deposit both tokens proportionally
//! and receive LP shares. Traders swap one token for the other, paying a 0.3% fee.
//!
//! ## Deposit Flow (atomic multi-action operation)
//!
//! For SOLEN:
//!   Action 1: Transfer(DEX_address, solen_amount)
//!   Action 2: Call(DEX, "deposit_solen")
//!
//! For STT (via STT token contract):
//!   Action 1: Call(STT, "transfer", DEX_address + amount)
//!   Action 2: Call(DEX, "deposit_stt", amount)
//!
//! ## Swap Flow
//!
//!   Call(DEX, "swap_solen_for_stt", amount_in)     — uses internal SOLEN balance
//!   Call(DEX, "swap_stt_for_solen", amount_in)     — uses internal STT balance
//!
//! ## Withdrawal Flow
//!
//!   Call(DEX, "withdraw_solen", amount)   — sends SOLEN via native transfer
//!   Call(DEX, "withdraw_stt", amount)     — credits STT to pending claims
//!
//! ## Storage Layout
//!
//! | Key | Value | Description |
//! |-----|-------|-------------|
//! | `owner` | `[u8; 32]` | Contract owner (deployer) |
//! | `stt_contract` | `[u8; 32]` | STT token contract address |
//! | `reserve_solen` | `u128` | Pool SOLEN reserve |
//! | `reserve_stt` | `u128` | Pool STT reserve |
//! | `total_lp` | `u128` | Total LP shares outstanding |
//! | `bal_solen/{account}` | `u128` | User's deposited SOLEN balance |
//! | `bal_stt/{account}` | `u128` | User's deposited STT balance |
//! | `lp/{account}` | `u128` | User's LP share balance |

#![no_std]

use solen_contract_sdk::{events, sdk, storage};

// ── Storage key builders ────────────────────────────────────────

fn solen_balance_key(account: &[u8; 32]) -> [u8; 42] {
    let mut key = [0u8; 42];
    key[..10].copy_from_slice(b"bal_solen/");
    key[10..].copy_from_slice(account);
    key
}

fn stt_balance_key(account: &[u8; 32]) -> [u8; 38] {
    let mut key = [0u8; 38];
    key[..6].copy_from_slice(b"bal_s/");
    key[6..].copy_from_slice(account);
    key
}

fn lp_key(account: &[u8; 32]) -> [u8; 35] {
    let mut key = [0u8; 35];
    key[..3].copy_from_slice(b"lp/");
    key[3..].copy_from_slice(account);
    key
}

// ── Storage helpers ─────────────────────────────────────────────

fn get_u128(key: &[u8]) -> u128 {
    storage::get_u128(key).unwrap_or(0)
}

fn set_u128(key: &[u8], val: u128) {
    storage::set_u128(key, val);
}

fn get_owner() -> [u8; 32] {
    let mut owner = [0u8; 32];
    if let Some(data) = storage::get(b"owner") {
        if data.len() >= 32 {
            owner.copy_from_slice(&data[..32]);
        }
    }
    owner
}

fn read_account(args: &[u8], offset: usize) -> Option<[u8; 32]> {
    if args.len() < offset + 32 { return None; }
    let mut account = [0u8; 32];
    account.copy_from_slice(&args[offset..offset + 32]);
    Some(account)
}

fn read_u128_arg(args: &[u8], offset: usize) -> Option<u128> {
    if args.len() < offset + 16 { return None; }
    let mut buf = [0u8; 16];
    buf.copy_from_slice(&args[offset..offset + 16]);
    Some(u128::from_le_bytes(buf))
}

// ── Swap math ───────────────────────────────────────────────────

/// Calculate output amount for a constant product AMM swap.
/// fee = 0.3% (3/1000)
/// amount_out = (reserve_out * amount_in * 997) / (reserve_in * 1000 + amount_in * 997)
fn get_amount_out(amount_in: u128, reserve_in: u128, reserve_out: u128) -> u128 {
    if amount_in == 0 || reserve_in == 0 || reserve_out == 0 {
        return 0;
    }
    let amount_in_with_fee = amount_in.saturating_mul(997);
    let numerator = amount_in_with_fee.saturating_mul(reserve_out);
    let denominator = reserve_in.saturating_mul(1000).saturating_add(amount_in_with_fee);
    if denominator == 0 { return 0; }
    numerator / denominator
}

/// Square root for initial LP token calculation (integer approximation).
fn isqrt(n: u128) -> u128 {
    if n == 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ── Entry point ─────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn call(input_ptr: i32, input_len: i32) -> i32 {
    let input = sdk::read_input(input_ptr, input_len);

    let null_pos = input.iter().position(|&b| b == 0).unwrap_or(input.len());
    let method = &input[..null_pos];
    let args = if null_pos + 1 < input.len() { &input[null_pos + 1..] } else { &[] };

    match method {
        b"abi" => do_abi(),
        b"init" => do_init(args),
        b"deposit_solen" => do_deposit_solen(args),
        b"deposit_stt" => do_deposit_stt(args),
        b"withdraw_solen" => do_withdraw_solen(args),
        b"withdraw_stt" => do_withdraw_stt(args),
        b"withdraw_all_solen" => do_withdraw_all_solen(),
        b"withdraw_all_stt" => do_withdraw_all_stt(),
        b"add_liquidity" => do_add_liquidity(args),
        b"remove_liquidity" => do_remove_liquidity(args),
        b"swap_solen_for_stt" => do_swap_solen_for_stt(args),
        b"swap_stt_for_solen" => do_swap_stt_for_solen(args),
        b"get_reserves" => do_get_reserves(),
        b"get_price" => do_get_price(args),
        b"balance_solen" => do_balance_solen(args),
        b"balance_stt" => do_balance_stt(args),
        b"balance_lp" => do_balance_lp(args),
        _ => sdk::return_value(b"unknown method"),
    }
}

// ── Method implementations ──────────────────────────────────────

fn do_abi() -> i32 {
    sdk::return_value(br#"{
"methods":[
{"name":"init","args":"stt_contract[32]","mutates":true},
{"name":"deposit_solen","args":"","mutates":true},
{"name":"deposit_stt","args":"amount[16]","mutates":true},
{"name":"withdraw_solen","args":"amount[16]","mutates":true},
{"name":"withdraw_stt","args":"amount[16]","mutates":true},
{"name":"add_liquidity","args":"solen_amount[16]+stt_amount[16]","mutates":true},
{"name":"remove_liquidity","args":"lp_amount[16]","mutates":true},
{"name":"swap_solen_for_stt","args":"amount_in[16]","mutates":true},
{"name":"swap_stt_for_solen","args":"amount_in[16]","mutates":true},
{"name":"get_reserves","args":"","mutates":false},
{"name":"get_price","args":"direction[1]","mutates":false},
{"name":"balance_solen","args":"account[32]","mutates":false},
{"name":"balance_stt","args":"account[32]","mutates":false},
{"name":"balance_lp","args":"account[32]","mutates":false}
],
"events":[
{"topic":"initialized","data":"owner[32]+stt_contract[32]"},
{"topic":"deposit","data":"token[1]+account[32]+amount[16]"},
{"topic":"withdraw","data":"token[1]+account[32]+amount[16]"},
{"topic":"swap","data":"direction[1]+amount_in[16]+amount_out[16]"},
{"topic":"liquidity_added","data":"solen[16]+stt[16]+lp_minted[16]"},
{"topic":"liquidity_removed","data":"solen[16]+stt[16]+lp_burned[16]"}
]
}"#)
}

fn do_init(args: &[u8]) -> i32 {
    let caller = sdk::caller();
    storage::set(b"owner", &caller);

    // Set STT contract address.
    if let Some(stt) = read_account(args, 0) {
        storage::set(b"stt_contract", &stt);
    } else {
        return sdk::return_value(b"err:need stt_contract[32]");
    }

    let mut event_data = [0u8; 64];
    event_data[..32].copy_from_slice(&caller);
    event_data[32..].copy_from_slice(&args[..32]);
    events::emit(b"initialized", &event_data);
    sdk::return_value(b"ok")
}

/// Record a SOLEN deposit. The actual SOLEN was transferred to this contract
/// in a preceding Transfer action within the same atomic operation.
/// The deposit amount is passed as args (u128 LE) for accounting.
fn do_deposit_solen(args: &[u8]) -> i32 {
    let caller = sdk::caller();
    let amount = if let Some(a) = read_u128_arg(args, 0) { a } else {
        return sdk::return_value(b"err:need amount[16]");
    };
    if amount == 0 { return sdk::return_value(b"err:zero amount"); }

    let key = solen_balance_key(&caller);
    let current = get_u128(&key);
    set_u128(&key, current.saturating_add(amount));

    let mut event_data = [0u8; 49];
    event_data[0] = 0; // 0 = SOLEN
    event_data[1..33].copy_from_slice(&caller);
    event_data[33..49].copy_from_slice(&amount.to_le_bytes());
    events::emit(b"deposit", &event_data);
    sdk::return_value(b"ok")
}

/// Record an STT deposit. The actual STT was transferred to this contract
/// via the STT token contract in a preceding Call action.
fn do_deposit_stt(args: &[u8]) -> i32 {
    let caller = sdk::caller();
    let amount = if let Some(a) = read_u128_arg(args, 0) { a } else {
        return sdk::return_value(b"err:need amount[16]");
    };
    if amount == 0 { return sdk::return_value(b"err:zero amount"); }

    let key = stt_balance_key(&caller);
    let current = get_u128(&key);
    set_u128(&key, current.saturating_add(amount));

    let mut event_data = [0u8; 49];
    event_data[0] = 1; // 1 = STT
    event_data[1..33].copy_from_slice(&caller);
    event_data[33..49].copy_from_slice(&amount.to_le_bytes());
    events::emit(b"deposit", &event_data);
    sdk::return_value(b"ok")
}

/// Withdraw SOLEN from the DEX back to the caller's wallet.
/// Uses the new transfer_native host function.
fn do_withdraw_solen(args: &[u8]) -> i32 {
    let caller = sdk::caller();
    let amount = if let Some(a) = read_u128_arg(args, 0) { a } else {
        return sdk::return_value(b"err:need amount[16]");
    };
    if amount == 0 { return sdk::return_value(b"err:zero amount"); }

    let key = solen_balance_key(&caller);
    let balance = get_u128(&key);
    if balance < amount {
        return sdk::return_value(b"err:insufficient_balance");
    }

    // Debit internal balance and send SOLEN via native transfer.
    set_u128(&key, balance - amount);
    if !sdk::transfer(&caller, amount) {
        // Revert the debit if transfer fails.
        set_u128(&key, balance);
        return sdk::return_value(b"err:transfer_failed");
    }

    let mut event_data = [0u8; 49];
    event_data[0] = 0;
    event_data[1..33].copy_from_slice(&caller);
    event_data[33..49].copy_from_slice(&amount.to_le_bytes());
    events::emit(b"withdraw", &event_data);
    sdk::return_value(b"ok")
}

/// Withdraw STT. Credits are tracked internally; the user claims via
/// a separate STT transfer_from call (the DEX owner approves a bulk allowance).
fn do_withdraw_stt(args: &[u8]) -> i32 {
    let caller = sdk::caller();
    let amount = if let Some(a) = read_u128_arg(args, 0) { a } else {
        return sdk::return_value(b"err:need amount[16]");
    };
    if amount == 0 { return sdk::return_value(b"err:zero amount"); }

    let key = stt_balance_key(&caller);
    let balance = get_u128(&key);
    if balance < amount {
        return sdk::return_value(b"err:insufficient_balance");
    }
    set_u128(&key, balance - amount);

    // Track pending STT claims. The contract owner periodically processes these
    // by calling STT.transfer on behalf of the DEX.
    let claim_key = {
        let mut k = [0u8; 38];
        k[..6].copy_from_slice(b"claim/");
        k[6..].copy_from_slice(&caller);
        k
    };
    let pending = get_u128(&claim_key);
    set_u128(&claim_key, pending.saturating_add(amount));

    let mut event_data = [0u8; 49];
    event_data[0] = 1;
    event_data[1..33].copy_from_slice(&caller);
    event_data[33..49].copy_from_slice(&amount.to_le_bytes());
    events::emit(b"withdraw", &event_data);
    sdk::return_value(b"ok")
}

/// Withdraw all SOLEN from the caller's internal balance.
fn do_withdraw_all_solen() -> i32 {
    let caller = sdk::caller();
    let key = solen_balance_key(&caller);
    let balance = get_u128(&key);
    if balance == 0 { return sdk::return_value(b"ok"); }

    set_u128(&key, 0);
    if !sdk::transfer(&caller, balance) {
        set_u128(&key, balance);
        return sdk::return_value(b"err:transfer_failed");
    }

    let mut event_data = [0u8; 49];
    event_data[0] = 0;
    event_data[1..33].copy_from_slice(&caller);
    event_data[33..49].copy_from_slice(&balance.to_le_bytes());
    events::emit(b"withdraw", &event_data);
    sdk::return_value(&balance.to_le_bytes())
}

/// Withdraw all STT from the caller's internal balance.
fn do_withdraw_all_stt() -> i32 {
    let caller = sdk::caller();
    let key = stt_balance_key(&caller);
    let balance = get_u128(&key);
    if balance == 0 { return sdk::return_value(b"ok"); }

    set_u128(&key, 0);

    let claim_key = {
        let mut k = [0u8; 38];
        k[..6].copy_from_slice(b"claim/");
        k[6..].copy_from_slice(&caller);
        k
    };
    let pending = get_u128(&claim_key);
    set_u128(&claim_key, pending.saturating_add(balance));

    let mut event_data = [0u8; 49];
    event_data[0] = 1;
    event_data[1..33].copy_from_slice(&caller);
    event_data[33..49].copy_from_slice(&balance.to_le_bytes());
    events::emit(b"withdraw", &event_data);
    sdk::return_value(&balance.to_le_bytes())
}

/// Add liquidity to the pool. Both SOLEN and STT amounts are taken from
/// the caller's internal DEX balances.
fn do_add_liquidity(args: &[u8]) -> i32 {
    let caller = sdk::caller();
    let solen_amount = if let Some(a) = read_u128_arg(args, 0) { a } else {
        return sdk::return_value(b"err:need solen_amount[16]+stt_amount[16]");
    };
    let stt_amount = if let Some(a) = read_u128_arg(args, 16) { a } else {
        return sdk::return_value(b"err:need solen_amount[16]+stt_amount[16]");
    };
    if solen_amount == 0 || stt_amount == 0 {
        return sdk::return_value(b"err:zero amounts");
    }

    // Check internal balances.
    let solen_key = solen_balance_key(&caller);
    let stt_key = stt_balance_key(&caller);
    let user_solen = get_u128(&solen_key);
    let user_stt = get_u128(&stt_key);

    if user_solen < solen_amount || user_stt < stt_amount {
        return sdk::return_value(b"err:insufficient_balance");
    }

    let reserve_solen = get_u128(b"reserve_solen");
    let reserve_stt = get_u128(b"reserve_stt");
    let total_lp = get_u128(b"total_lp");

    // Calculate LP tokens to mint and actual amounts used.
    let (lp_minted, actual_solen, actual_stt) = if total_lp == 0 {
        // First liquidity provision — LP = sqrt(solen * stt).
        let product = solen_amount.saturating_mul(stt_amount);
        let lp = isqrt(product);
        if lp == 0 { return sdk::return_value(b"err:liquidity_too_small"); }
        (lp, solen_amount, stt_amount)
    } else {
        // Proportional to existing reserves. Only use what maintains the ratio.
        let lp_from_solen = solen_amount.saturating_mul(total_lp) / reserve_solen;
        let lp_from_stt = stt_amount.saturating_mul(total_lp) / reserve_stt;
        if lp_from_solen < lp_from_stt {
            // SOLEN is the limiting factor — scale STT down.
            let used_stt = lp_from_solen.saturating_mul(reserve_stt) / total_lp;
            (lp_from_solen, solen_amount, used_stt)
        } else {
            // STT is the limiting factor — scale SOLEN down.
            let used_solen = lp_from_stt.saturating_mul(reserve_solen) / total_lp;
            (lp_from_stt, used_solen, stt_amount)
        }
    };

    if lp_minted == 0 {
        return sdk::return_value(b"err:insufficient_liquidity");
    }

    // Debit only the amounts actually used. Excess stays in user's DEX balance.
    set_u128(&solen_key, user_solen - actual_solen);
    set_u128(&stt_key, user_stt - actual_stt);
    set_u128(b"reserve_solen", reserve_solen.saturating_add(actual_solen));
    set_u128(b"reserve_stt", reserve_stt.saturating_add(actual_stt));
    set_u128(b"total_lp", total_lp.saturating_add(lp_minted));

    let lp_k = lp_key(&caller);
    let user_lp = get_u128(&lp_k);
    set_u128(&lp_k, user_lp.saturating_add(lp_minted));

    let mut event_data = [0u8; 48];
    event_data[..16].copy_from_slice(&actual_solen.to_le_bytes());
    event_data[16..32].copy_from_slice(&actual_stt.to_le_bytes());
    event_data[32..48].copy_from_slice(&lp_minted.to_le_bytes());
    events::emit(b"liquidity_added", &event_data);
    sdk::return_value(&lp_minted.to_le_bytes())
}

/// Remove liquidity. Burns LP tokens, returns proportional SOLEN and STT
/// to the caller's internal balances.
fn do_remove_liquidity(args: &[u8]) -> i32 {
    let caller = sdk::caller();
    let lp_amount = if let Some(a) = read_u128_arg(args, 0) { a } else {
        return sdk::return_value(b"err:need lp_amount[16]");
    };
    if lp_amount == 0 { return sdk::return_value(b"err:zero amount"); }

    let lp_k = lp_key(&caller);
    let user_lp = get_u128(&lp_k);
    if user_lp < lp_amount {
        return sdk::return_value(b"err:insufficient_lp");
    }

    let reserve_solen = get_u128(b"reserve_solen");
    let reserve_stt = get_u128(b"reserve_stt");
    let total_lp = get_u128(b"total_lp");

    if total_lp == 0 { return sdk::return_value(b"err:no_liquidity"); }

    let solen_out = lp_amount.saturating_mul(reserve_solen) / total_lp;
    let stt_out = lp_amount.saturating_mul(reserve_stt) / total_lp;

    // Burn LP, update reserves, credit user.
    set_u128(&lp_k, user_lp - lp_amount);
    set_u128(b"total_lp", total_lp - lp_amount);
    set_u128(b"reserve_solen", reserve_solen.saturating_sub(solen_out));
    set_u128(b"reserve_stt", reserve_stt.saturating_sub(stt_out));

    let solen_key = solen_balance_key(&caller);
    let stt_key = stt_balance_key(&caller);
    set_u128(&solen_key, get_u128(&solen_key).saturating_add(solen_out));
    set_u128(&stt_key, get_u128(&stt_key).saturating_add(stt_out));

    let mut event_data = [0u8; 48];
    event_data[..16].copy_from_slice(&solen_out.to_le_bytes());
    event_data[16..32].copy_from_slice(&stt_out.to_le_bytes());
    event_data[32..48].copy_from_slice(&lp_amount.to_le_bytes());
    events::emit(b"liquidity_removed", &event_data);

    // Return both amounts as the output.
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&solen_out.to_le_bytes());
    out[16..].copy_from_slice(&stt_out.to_le_bytes());
    sdk::return_value(&out)
}

/// Swap SOLEN for STT using the AMM.
fn do_swap_solen_for_stt(args: &[u8]) -> i32 {
    let caller = sdk::caller();
    let amount_in = if let Some(a) = read_u128_arg(args, 0) { a } else {
        return sdk::return_value(b"err:need amount_in[16]");
    };
    if amount_in == 0 { return sdk::return_value(b"err:zero amount"); }

    let solen_key = solen_balance_key(&caller);
    let user_solen = get_u128(&solen_key);
    if user_solen < amount_in {
        return sdk::return_value(b"err:insufficient_balance");
    }

    let reserve_solen = get_u128(b"reserve_solen");
    let reserve_stt = get_u128(b"reserve_stt");

    let amount_out = get_amount_out(amount_in, reserve_solen, reserve_stt);
    if amount_out == 0 {
        return sdk::return_value(b"err:insufficient_liquidity");
    }

    // Update balances and reserves.
    set_u128(&solen_key, user_solen - amount_in);
    let stt_key = stt_balance_key(&caller);
    set_u128(&stt_key, get_u128(&stt_key).saturating_add(amount_out));
    set_u128(b"reserve_solen", reserve_solen.saturating_add(amount_in));
    set_u128(b"reserve_stt", reserve_stt.saturating_sub(amount_out));

    let mut event_data = [0u8; 33];
    event_data[0] = 0; // direction: SOLEN -> STT
    event_data[1..17].copy_from_slice(&amount_in.to_le_bytes());
    event_data[17..33].copy_from_slice(&amount_out.to_le_bytes());
    events::emit(b"swap", &event_data);
    sdk::return_value(&amount_out.to_le_bytes())
}

/// Swap STT for SOLEN using the AMM.
fn do_swap_stt_for_solen(args: &[u8]) -> i32 {
    let caller = sdk::caller();
    let amount_in = if let Some(a) = read_u128_arg(args, 0) { a } else {
        return sdk::return_value(b"err:need amount_in[16]");
    };
    if amount_in == 0 { return sdk::return_value(b"err:zero amount"); }

    let stt_key = stt_balance_key(&caller);
    let user_stt = get_u128(&stt_key);
    if user_stt < amount_in {
        return sdk::return_value(b"err:insufficient_balance");
    }

    let reserve_solen = get_u128(b"reserve_solen");
    let reserve_stt = get_u128(b"reserve_stt");

    let amount_out = get_amount_out(amount_in, reserve_stt, reserve_solen);
    if amount_out == 0 {
        return sdk::return_value(b"err:insufficient_liquidity");
    }

    // Update balances and reserves.
    set_u128(&stt_key, user_stt - amount_in);
    let solen_key = solen_balance_key(&caller);
    set_u128(&solen_key, get_u128(&solen_key).saturating_add(amount_out));
    set_u128(b"reserve_stt", reserve_stt.saturating_add(amount_in));
    set_u128(b"reserve_solen", reserve_solen.saturating_sub(amount_out));

    let mut event_data = [0u8; 33];
    event_data[0] = 1; // direction: STT -> SOLEN
    event_data[1..17].copy_from_slice(&amount_in.to_le_bytes());
    event_data[17..33].copy_from_slice(&amount_out.to_le_bytes());
    events::emit(b"swap", &event_data);
    sdk::return_value(&amount_out.to_le_bytes())
}

// ── View methods ────────────────────────────────────────────────

fn do_get_reserves() -> i32 {
    let reserve_solen = get_u128(b"reserve_solen");
    let reserve_stt = get_u128(b"reserve_stt");
    let total_lp = get_u128(b"total_lp");
    let mut out = [0u8; 48];
    out[..16].copy_from_slice(&reserve_solen.to_le_bytes());
    out[16..32].copy_from_slice(&reserve_stt.to_le_bytes());
    out[32..48].copy_from_slice(&total_lp.to_le_bytes());
    sdk::return_value(&out)
}

/// Get price: direction 0 = SOLEN->STT, 1 = STT->SOLEN.
/// Returns the amount of output per 1 unit (1e8 base units) of input.
fn do_get_price(args: &[u8]) -> i32 {
    let direction = if !args.is_empty() { args[0] } else { 0 };
    let reserve_solen = get_u128(b"reserve_solen");
    let reserve_stt = get_u128(b"reserve_stt");

    let one_unit: u128 = 100_000_000; // 1 token = 1e8 base units (8 decimals)
    let price = if direction == 0 {
        get_amount_out(one_unit, reserve_solen, reserve_stt)
    } else {
        get_amount_out(one_unit, reserve_stt, reserve_solen)
    };
    sdk::return_value(&price.to_le_bytes())
}

fn do_balance_solen(args: &[u8]) -> i32 {
    let account = if let Some(a) = read_account(args, 0) { a } else {
        return sdk::return_value(b"err:need account[32]");
    };
    let balance = get_u128(&solen_balance_key(&account));
    sdk::return_value(&balance.to_le_bytes())
}

fn do_balance_stt(args: &[u8]) -> i32 {
    let account = if let Some(a) = read_account(args, 0) { a } else {
        return sdk::return_value(b"err:need account[32]");
    };
    let balance = get_u128(&stt_balance_key(&account));
    sdk::return_value(&balance.to_le_bytes())
}

fn do_balance_lp(args: &[u8]) -> i32 {
    let account = if let Some(a) = read_account(args, 0) { a } else {
        return sdk::return_value(b"err:need account[32]");
    };
    let balance = get_u128(&lp_key(&account));
    sdk::return_value(&balance.to_le_bytes())
}
