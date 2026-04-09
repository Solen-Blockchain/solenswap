// SolenSwap — STT/SOLEN AMM Trading Interface
// Connects to Solen blockchain via JSON-RPC + browser extension wallet.

const CONFIG = {
  rpc: 'https://testnet-rpc3.solenchain.io',
  chainId: 9000,
  dexContract: '7ab62cea689710f8e219139aceb7f811b1e488775563d6828aedc5648a492639',
  sttContract: 'Dse2ppCqGpFrrUpuKFuXtSVQiBq8mvLF2cU8npQWicmp',
  decimals: 8,
};

const BASE = 10 ** CONFIG.decimals;

// ===== Encoding Helpers ====================================================

const B58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

function base58ToHex(b58) {
  let n = BigInt(0);
  for (const c of b58) {
    const idx = B58_ALPHABET.indexOf(c);
    if (idx < 0) throw new Error('invalid base58');
    n = n * 58n + BigInt(idx);
  }
  return n.toString(16).padStart(64, '0');
}

function hexToBase58(hex) {
  let n = BigInt('0x' + hex);
  if (n === 0n) return '1';
  let s = '';
  while (n > 0n) {
    s = B58_ALPHABET[Number(n % 58n)] + s;
    n = n / 58n;
  }
  return s;
}

function hexToBytes(hex) {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < hex.length; i += 2)
    bytes[i / 2] = parseInt(hex.substr(i, 2), 16);
  return bytes;
}

function bytesToHex(bytes) {
  return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('');
}

function u128ToLeHex(n) {
  const buf = new ArrayBuffer(16);
  const view = new DataView(buf);
  const big = BigInt(n);
  view.setBigUint64(0, big & 0xFFFFFFFFFFFFFFFFn, true);
  view.setBigUint64(8, big >> 64n, true);
  return bytesToHex(new Uint8Array(buf));
}

function leHexToU128(hex) {
  if (!hex || hex.length < 32) hex = (hex || '').padEnd(32, '0');
  const bytes = hexToBytes(hex);
  const view = new DataView(bytes.buffer);
  const lo = view.getBigUint64(0, true);
  const hi = view.getBigUint64(8, true);
  return lo + (hi << 64n);
}

function formatAmount(baseUnits, decimals) {
  decimals = decimals || CONFIG.decimals;
  const n = Number(baseUnits) / (10 ** decimals);
  if (n >= 1e6) return (n / 1e6).toFixed(2) + 'M';
  if (n >= 1e3) return n.toLocaleString(undefined, { maximumFractionDigits: 2 });
  if (n >= 1) return n.toFixed(4);
  if (n >= 0.0001) return n.toFixed(6);
  return n.toFixed(8);
}

function parseAmount(str) {
  const n = parseFloat(str);
  if (isNaN(n) || n <= 0) return 0n;
  return BigInt(Math.round(n * BASE));
}

// ===== RPC =================================================================

async function rpc(method, params) {
  const resp = await fetch(CONFIG.rpc, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
  });
  const data = await resp.json();
  if (data.error) throw new Error(data.error.message);
  return data.result;
}

async function callView(target, method, argsHex) {
  const result = await rpc('solen_callView', [target, method, argsHex || '']);
  return result?.return_data || '';
}

// ===== Wallet ==============================================================

let wallet = null; // { accountBase58, accountHex, provider }

async function connectWallet() {
  if (!window.solen) {
    alert('Install the Solen Browser Extension to connect.');
    return;
  }
  try {
    const accounts = await window.solen.connect();
    if (accounts && accounts.length > 0) {
      wallet = {
        accountBase58: accounts[0],
        accountHex: base58ToHex(accounts[0]),
        provider: window.solen,
      };
      onWalletConnected();
    }
  } catch (e) {
    console.warn('Connect failed:', e);
  }
}

function onWalletConnected() {
  document.getElementById('wallet-disconnected').classList.add('hidden');
  document.getElementById('wallet-connected').classList.remove('hidden');
  const addr = wallet.accountBase58;
  document.getElementById('wallet-addr').textContent = addr.slice(0, 8) + '...' + addr.slice(-6);
  loadBalances();
  updateSwapEstimate();
}

function disconnectWallet() {
  wallet = null;
  document.getElementById('wallet-disconnected').classList.remove('hidden');
  document.getElementById('wallet-connected').classList.add('hidden');
  updateSwapEstimate();
}

// Auto-connect
window.addEventListener('solen#initialized', async () => {
  if (window.solen) {
    try {
      const accounts = await window.solen.getAccounts();
      if (accounts && accounts.length > 0) {
        wallet = { accountBase58: accounts[0], accountHex: base58ToHex(accounts[0]), provider: window.solen };
        onWalletConnected();
      }
    } catch {}
  }
});
setTimeout(async () => {
  if (window.solen && !wallet) {
    try {
      const accounts = await window.solen.getAccounts();
      if (accounts && accounts.length > 0) {
        wallet = { accountBase58: accounts[0], accountHex: base58ToHex(accounts[0]), provider: window.solen };
        onWalletConnected();
      }
    } catch {}
  }
}, 500);

document.getElementById('wallet-connect-btn').addEventListener('click', connectWallet);
document.getElementById('wallet-disconnect-btn').addEventListener('click', disconnectWallet);

// ===== Pool Data ===========================================================

let poolData = { reserveSolen: 0n, reserveStt: 0n, totalLp: 0n };

async function loadPoolData() {
  try {
    const hex = await callView(CONFIG.dexContract, 'get_reserves');
    if (hex && hex.length >= 96) {
      poolData.reserveSolen = leHexToU128(hex.slice(0, 32));
      poolData.reserveStt = leHexToU128(hex.slice(32, 64));
      poolData.totalLp = leHexToU128(hex.slice(64, 96));
    }
  } catch (e) {
    console.warn('Failed to load pool data:', e);
  }

  document.getElementById('reserve-solen').textContent = formatAmount(poolData.reserveSolen) + ' SOLEN';
  document.getElementById('reserve-stt').textContent = formatAmount(poolData.reserveStt) + ' STT';
  document.getElementById('total-lp').textContent = formatAmount(poolData.totalLp);

  if (poolData.reserveSolen > 0n && poolData.reserveStt > 0n) {
    const price = Number(poolData.reserveStt) / Number(poolData.reserveSolen);
    document.getElementById('price').textContent = '1 SOLEN = ' + price.toFixed(4) + ' STT';
  } else {
    document.getElementById('price').textContent = 'No liquidity';
  }
}

// ===== User Balances =======================================================

let walletSolen = 0n;
let walletStt = 0n;
let userLp = 0n;

async function loadBalances() {
  if (!wallet) return;

  try {
    const acct = await rpc('solen_getAccount', [wallet.accountBase58]);
    if (acct) {
      walletSolen = BigInt(acct.balance || '0');
      document.getElementById('w-solen').textContent = formatAmount(walletSolen) + ' SOLEN';
      document.getElementById('liq-solen-bal').textContent = formatAmount(walletSolen);
      if (swapDirection === 0) document.getElementById('swap-balance').textContent = formatAmount(walletSolen) + ' SOLEN';
    }
  } catch {}

  try {
    const hex = await callView(CONFIG.sttContract, 'balance_of', wallet.accountHex);
    walletStt = leHexToU128(hex);
    document.getElementById('w-stt').textContent = formatAmount(walletStt) + ' STT';
    document.getElementById('liq-stt-bal').textContent = formatAmount(walletStt);
    if (swapDirection === 1) document.getElementById('swap-balance').textContent = formatAmount(walletStt) + ' STT';
  } catch {}

  try {
    const hex = await callView(CONFIG.dexContract, 'balance_lp', wallet.accountHex);
    userLp = leHexToU128(hex);
    document.getElementById('w-lp').textContent = formatAmount(userLp);
    document.getElementById('liq-lp-bal').textContent = formatAmount(userLp);
  } catch {}
}

// ===== Helpers =============================================================

function showStatus(elId, msg, type) {
  const el = document.getElementById(elId);
  el.textContent = msg;
  el.className = 'status' + (type ? ' ' + type : '');
}

// ===== Swap Math ===========================================================

let swapDirection = 0; // 0 = SOLEN->STT, 1 = STT->SOLEN

function getAmountOut(amountIn, reserveIn, reserveOut) {
  if (amountIn === 0n || reserveIn === 0n || reserveOut === 0n) return 0n;
  const amountInWithFee = amountIn * 997n;
  const numerator = amountInWithFee * reserveOut;
  const denominator = reserveIn * 1000n + amountInWithFee;
  if (denominator === 0n) return 0n;
  return numerator / denominator;
}

function updateSwapEstimate() {
  const input = document.getElementById('swap-input').value;
  const amountIn = parseAmount(input);
  const btn = document.getElementById('swap-btn');

  if (amountIn === 0n) {
    document.getElementById('swap-output').value = '';
    document.getElementById('swap-rate').textContent = '--';
    document.getElementById('swap-impact').textContent = '--';
    document.getElementById('swap-fee').textContent = '--';
    btn.textContent = 'Enter an amount';
    btn.disabled = true;
    return;
  }

  const [reserveIn, reserveOut] = swapDirection === 0
    ? [poolData.reserveSolen, poolData.reserveStt]
    : [poolData.reserveStt, poolData.reserveSolen];

  const amountOut = getAmountOut(amountIn, reserveIn, reserveOut);

  if (amountOut === 0n) {
    document.getElementById('swap-output').value = 'Insufficient liquidity';
    btn.textContent = 'Insufficient liquidity';
    btn.disabled = true;
    return;
  }

  document.getElementById('swap-output').value = formatAmount(amountOut);

  const inToken = swapDirection === 0 ? 'SOLEN' : 'STT';
  const outToken = swapDirection === 0 ? 'STT' : 'SOLEN';
  const rate = Number(amountOut) / Number(amountIn);
  document.getElementById('swap-rate').textContent = `1 ${inToken} = ${rate.toFixed(4)} ${outToken}`;

  const idealOut = Number(amountIn) * Number(reserveOut) / Number(reserveIn);
  const impact = idealOut > 0 ? ((idealOut - Number(amountOut)) / idealOut * 100) : 0;
  document.getElementById('swap-impact').textContent = impact.toFixed(2) + '%';

  const fee = Number(amountIn) * 0.003;
  document.getElementById('swap-fee').textContent = formatAmount(BigInt(Math.round(fee))) + ' ' + inToken;

  btn.textContent = `Swap ${inToken} for ${outToken}`;
  btn.disabled = !wallet;
}

// ===== Tabs ================================================================

document.querySelectorAll('.tab').forEach(tab => {
  tab.addEventListener('click', () => {
    document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
    tab.classList.add('active');
    document.querySelectorAll('.panel').forEach(p => p.classList.add('hidden'));
    document.getElementById('panel-' + tab.dataset.tab).classList.remove('hidden');
  });
});

// ===== Swap Direction ======================================================

document.getElementById('dir-solen-stt').addEventListener('click', () => {
  swapDirection = 0;
  document.getElementById('dir-solen-stt').classList.add('active');
  document.getElementById('dir-stt-solen').classList.remove('active');
  document.getElementById('swap-input-token').textContent = 'SOLEN';
  document.getElementById('swap-output-token').textContent = 'STT';
  document.getElementById('swap-input').value = '';
  document.getElementById('swap-output').value = '';
  updateSwapEstimate();
  loadBalances();
});

document.getElementById('dir-stt-solen').addEventListener('click', () => {
  swapDirection = 1;
  document.getElementById('dir-stt-solen').classList.add('active');
  document.getElementById('dir-solen-stt').classList.remove('active');
  document.getElementById('swap-input-token').textContent = 'STT';
  document.getElementById('swap-output-token').textContent = 'SOLEN';
  document.getElementById('swap-input').value = '';
  document.getElementById('swap-output').value = '';
  updateSwapEstimate();
  loadBalances();
});

document.getElementById('swap-input').addEventListener('input', updateSwapEstimate);

// ===== Swap Action =========================================================

document.getElementById('swap-btn').addEventListener('click', async () => {
  if (!wallet) return;
  const amountIn = parseAmount(document.getElementById('swap-input').value);
  if (amountIn === 0n) return;

  const dexBase58 = hexToBase58(CONFIG.dexContract);
  const amountStr = (Number(amountIn) / BASE).toString();
  const amountHex = u128ToLeHex(amountIn);

  showStatus('swap-status', 'Signing transaction...', '');
  try {
    if (swapDirection === 0) {
      // SOLEN -> STT: Transfer SOLEN to DEX + deposit + swap (output stays in DEX balance)
      await wallet.provider.signAndSubmit({
        actions: [
          { type: 'transfer', to: dexBase58, amount: amountStr },
          { type: 'call', target: dexBase58, method: 'deposit_solen', args: amountHex },
          { type: 'call', target: dexBase58, method: 'swap_solen_for_stt', args: amountHex },
        ]
      });
    } else {
      // STT -> SOLEN: Transfer STT via token contract + deposit + swap
      const transferArgs = CONFIG.dexContract + amountHex;
      await wallet.provider.signAndSubmit({
        actions: [
          { type: 'call', target: CONFIG.sttContract, method: 'transfer', args: transferArgs },
          { type: 'call', target: dexBase58, method: 'deposit_stt', args: amountHex },
          { type: 'call', target: dexBase58, method: 'swap_stt_for_solen', args: amountHex },
        ]
      });
    }
    showStatus('swap-status', 'Swap successful!', 'success');
    document.getElementById('swap-input').value = '';
    document.getElementById('swap-output').value = '';
    setTimeout(() => { loadPoolData(); loadBalances(); }, 4000);
  } catch (e) {
    showStatus('swap-status', e.message, 'error');
  }
});


// ===== Liquidity Actions ===================================================

document.getElementById('liq-add-btn').addEventListener('click', async () => {
  if (!wallet) return;
  const solenAmt = parseAmount(document.getElementById('liq-solen').value);
  const sttAmt = parseAmount(document.getElementById('liq-stt').value);
  if (solenAmt === 0n || sttAmt === 0n) return;

  const dexBase58 = hexToBase58(CONFIG.dexContract);
  const solenStr = (Number(solenAmt) / BASE).toString();
  const sttTransferArgs = CONFIG.dexContract + u128ToLeHex(sttAmt);
  const addLiqArgs = u128ToLeHex(solenAmt) + u128ToLeHex(sttAmt);

  showStatus('liq-status', 'Signing add liquidity...', '');
  try {
    // Atomic: Transfer SOLEN + Transfer STT + deposit both + add liquidity
    await wallet.provider.signAndSubmit({
      actions: [
        { type: 'transfer', to: dexBase58, amount: solenStr },
        { type: 'call', target: CONFIG.sttContract, method: 'transfer', args: sttTransferArgs },
        { type: 'call', target: dexBase58, method: 'deposit_solen', args: u128ToLeHex(solenAmt) },
        { type: 'call', target: dexBase58, method: 'deposit_stt', args: u128ToLeHex(sttAmt) },
        { type: 'call', target: dexBase58, method: 'add_liquidity', args: addLiqArgs },
      ]
    });
    showStatus('liq-status', 'Liquidity added!', 'success');
    document.getElementById('liq-solen').value = '';
    document.getElementById('liq-stt').value = '';
    setTimeout(() => { loadPoolData(); loadBalances(); }, 4000);
  } catch (e) {
    showStatus('liq-status', e.message, 'error');
  }
});

document.getElementById('liq-remove-btn').addEventListener('click', async () => {
  if (!wallet) return;
  const lpAmt = parseAmount(document.getElementById('liq-remove').value);
  if (lpAmt === 0n) return;

  const dexBase58 = hexToBase58(CONFIG.dexContract);

  showStatus('liq-status', 'Signing remove liquidity...', '');
  try {
    // Remove liquidity — returns tokens to DEX internal balance.
    // Then withdraw both back to wallet.
    await wallet.provider.signAndSubmit({
      actions: [
        { type: 'call', target: dexBase58, method: 'remove_liquidity', args: u128ToLeHex(lpAmt) },
      ]
    });
    showStatus('liq-status', 'Liquidity removed! Tokens returned to DEX balance.', 'success');
    document.getElementById('liq-remove').value = '';
    setTimeout(() => { loadPoolData(); loadBalances(); }, 4000);
  } catch (e) {
    showStatus('liq-status', e.message, 'error');
  }
});

// ===== Input Enable/Disable ================================================

['liq-solen', 'liq-stt'].forEach(id => {
  document.getElementById(id).addEventListener('input', () => {
    const s = parseFloat(document.getElementById('liq-solen').value);
    const t = parseFloat(document.getElementById('liq-stt').value);
    document.getElementById('liq-add-btn').disabled = !wallet || !s || !t || s <= 0 || t <= 0;
  });
});

document.getElementById('liq-remove').addEventListener('input', () => {
  const v = parseFloat(document.getElementById('liq-remove').value);
  document.getElementById('liq-remove-btn').disabled = !wallet || !v || v <= 0;
});

// ===== Init ================================================================

loadPoolData();
setInterval(loadPoolData, 10000);
setInterval(() => { if (wallet) loadBalances(); }, 10000);
