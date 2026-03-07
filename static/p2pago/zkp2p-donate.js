(function (global, factory) {
    typeof exports === 'object' && typeof module !== 'undefined' ? factory(exports, require('ethers')) :
    typeof define === 'function' && define.amd ? define(['exports', 'ethers'], factory) :
    (global = typeof globalThis !== 'undefined' ? globalThis : global || self, factory(global.Zkp2pDonate = {}, global.ethers));
})(this, (function (exports, ethers) { 'use strict';

    /**
     * ZKP2P Donate SDK constants
     * Base mainnet values; override via config when needed.
     */
    /** ZKP2P Escrow contract address on Base */
    const ESCROW_ADDRESS = '0xCA38607D85E8F6294Dc10728669605E6664C2D70';
    /** Base chain ID */
    const BASE_CHAIN_ID = 8453;
    /** ZKP2P API base URL (v1) */
    const ZKP2P_API_BASE_URL = 'https://api.zkp2p.xyz/v1';
    /** Default Ethereum mainnet RPC URL used for ENS resolution when no provider is passed */
    const DEFAULT_MAINNET_RPC_URL = 'https://ethereum.publicnode.com';
    /** PeerAuth extension Chrome Web Store install URL */
    const ZKP2P_EXTENSION_INSTALL_URL = 'https://chromewebstore.google.com/detail/zkp2p-extension/ijpgccednehjpeclfcllnjjcmiohdjih';
    /** Default recipient when no app recipient specified (FluidKey ENS) */
    const P2PAGO_DEFAULT_RECIPIENT = 'p2pago.fkey.id';
    /** Default referrer string for Peer extension onramp (attribution/display) */
    const P2PAGO_DEFAULT_REFERRER = 'p2pago';
    /** Fee parameters. Reserved for future use. */
    const P2PAGO_FEE_PERCENT = 0.01;
    const P2PAGO_FEE_MIN_USD = 0.1;
    /** Maximum allowed gas cost as fraction of donation amount. */
    const GAS_COST_MAX_FRACTION = 0.5;
    /** Amount threshold below which a small-donation warning may be shown. */
    const MIN_DONATION_WARNING_USD = 2;

    /**
     * Default RPC provider for ENS resolution (mainnet).
     * Used when callers do not pass a provider. Lazy singleton; ethers v5/v6 compatible.
     */
    let cached = null;
    /**
     * Return a mainnet provider suitable for ENS resolution when no provider is passed.
     * Uses ethers (v5 or v6) if available; returns null otherwise so callers can throw
     * a clear error. Cached after first successful creation.
     */
    async function getDefaultProvider() {
        if (cached)
            return cached;
        try {
            const m = await import('ethers');
            const ethers = m.default ?? m;
            const RpcProvider = ethers.JsonRpcProvider ??
                ethers.providers?.JsonRpcProvider;
            if (!RpcProvider)
                return null;
            cached = new RpcProvider(DEFAULT_MAINNET_RPC_URL);
            return cached;
        }
        catch {
            return null;
        }
    }

    /**
     * Address resolution — ENS (including FluidKey) or raw 0x address
     */
    const HEX_REGEX = /^0x[a-fA-F0-9]{40}$/;
    /** Check if input looks like a raw Ethereum address */
    function isAddress(value) {
        return HEX_REGEX.test(value);
    }
    /**
     * Resolve recipient to address. If ENS (e.g. FluidKey *.fkey.eth), resolve via provider.
     * When no provider is passed, uses a default mainnet provider (requires ethers).
     * With FluidKey, each resolution returns a unique stealth address for recipient privacy.
     */
    async function resolveAddress(recipient, provider) {
        if (isAddress(recipient)) {
            return recipient;
        }
        const effectiveProvider = provider ?? (await getDefaultProvider());
        if (!effectiveProvider) {
            throw new Error('ENS resolution requires a provider. Pass provider in options when recipient is an ENS name (e.g. myapp.fkey.eth), or ensure ethers is installed for default mainnet resolution.');
        }
        let resolved = null;
        if (effectiveProvider.resolveName) {
            resolved = await effectiveProvider.resolveName(recipient);
        }
        else if (effectiveProvider.getResolver) {
            const resolver = await effectiveProvider.getResolver(recipient);
            if (resolver?.resolve) {
                resolved = await resolver.resolve(recipient);
            }
        }
        if (!resolved) {
            throw new Error(`Failed to resolve ENS name: ${recipient}`);
        }
        return resolved;
    }
    /**
     * Resolve recipient to a 0x address. Input may be ENS (e.g. p2pago.fkey.id) or already a 0x address.
     * When no provider is passed, uses SDK default mainnet provider (requires ethers).
     */
    async function resolveRecipient(recipient, options = {}) {
        return resolveAddress(recipient, options.provider);
    }

    /**
     * ZKP2P REST API adapter
     */
    /** Convert USD amount to 6-decimal string (e.g. 5 -> "5000000") */
    function toExactFiatAmount(amountUsd) {
        return Math.round(amountUsd * 1000000).toString();
    }
    /**
     * Fetch quote from ZKP2P /quote/exact-fiat
     */
    async function fetchQuote(config, params) {
        const baseUrl = config.baseUrl ?? ZKP2P_API_BASE_URL;
        const chainId = params.chainId ?? 8453;
        const destinationToken = params.destinationToken ?? '0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913';
        const paymentPlatforms = params.platform
            ? [params.platform.toLowerCase()]
            : ['venmo', 'cashapp'];
        const body = {
            paymentPlatforms,
            fiatCurrency: 'USD',
            user: params.userAddress,
            recipient: params.recipient,
            destinationChainId: chainId,
            destinationToken,
            exactFiatAmount: toExactFiatAmount(params.amountUsd),
        };
        const res = await fetch(`${baseUrl}/quote/exact-fiat`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
        });
        if (!res.ok) {
            const text = await res.text();
            throw new Error(`ZKP2P quote failed: ${res.status} ${text}`);
        }
        const json = (await res.json());
        if (!json.success || !json.responseObject?.quotes?.length) {
            throw new Error('ZKP2P quote returned no quotes');
        }
        const first = json.responseObject.quotes[0];
        const quoteItem = {
            fiatAmount: first.fiatAmount,
            fiatAmountFormatted: first.fiatAmountFormatted,
            tokenAmount: first.tokenAmount,
            tokenAmountFormatted: first.tokenAmountFormatted,
            paymentMethod: first.paymentMethod,
            payeeAddress: first.payeeAddress,
            conversionRate: first.conversionRate,
            intent: first.intent,
        };
        return {
            quotes: [quoteItem],
            intent: first.intent,
            payeeAddress: first.payeeAddress,
            platform: first.paymentMethod,
            fiatAmountFormatted: first.fiatAmountFormatted,
            tokenAmountFormatted: first.tokenAmountFormatted,
        };
    }
    /**
     * Verify intent via ZKP2P /verify/intent (call from backend; apiKey stays on server)
     */
    async function verifyIntent$1(config, intent, apiKey) {
        const baseUrl = config.baseUrl ?? ZKP2P_API_BASE_URL;
        const body = {
            processorName: intent.processorName,
            depositId: intent.depositId,
            tokenAmount: intent.amount,
            payeeDetails: intent.payeeDetails,
            toAddress: intent.toAddress,
            fiatCurrencyCode: intent.fiatCurrencyCode,
            chainId: intent.chainId,
        };
        const res = await fetch(`${baseUrl}/verify/intent`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'x-api-key': apiKey,
            },
            body: JSON.stringify(body),
        });
        if (!res.ok) {
            const text = await res.text();
            throw new Error(`ZKP2P verify intent failed: ${res.status} ${text}`);
        }
        const json = (await res.json());
        if (!json.success || !json.responseObject) {
            throw new Error('ZKP2P verify intent failed');
        }
        return {
            signedIntent: json.responseObject.signedIntent,
            intentData: json.responseObject.intentData,
        };
    }

    /**
     * getQuote — fetch ZKP2P quote for exact fiat amount
     */
    /**
     * Get a quote for donating exact fiat amount via ZKP2P.
     * Resolves recipient (ENS or 0x) before calling API — supports FluidKey for recipient privacy.
     */
    async function getQuote(options) {
        const { recipient, amountUsd, userAddress, platform, chainId, destinationToken, provider, apiBaseUrl, } = options;
        const resolvedRecipient = await resolveAddress(recipient, provider);
        const config = { baseUrl: apiBaseUrl ?? ZKP2P_API_BASE_URL };
        return fetchQuote(config, {
            recipient: resolvedRecipient,
            amountUsd,
            platform,
            chainId,
            destinationToken,
            userAddress,
        });
    }

    /**
     * verifyIntent — call ZKP2P /verify/intent (for serverless/backend)
     * API key should stay on server; this is optional for apps that pass key from backend.
     */
    /**
     * Verify intent via ZKP2P API. Typically called from your backend (API key stays on server).
     * Exposed for serverless/edge setups where the app passes the key.
     */
    async function verifyIntent(intent, options) {
        const config = { baseUrl: options.apiBaseUrl ?? ZKP2P_API_BASE_URL };
        return verifyIntent$1(config, intent, options.apiKey);
    }

    /**
     * ZKP2P Escrow contract adapter
     * Uses ethers Contract; signer should be from ethers (e.g. provider.getSigner()).
     */
    const ESCROW_ABI = [
        'function signalIntent(uint256 _depositId, uint256 _amount, address _to, address _paymentVerifier, bytes32 _fiatCurrency, bytes calldata _gatingServiceSignature) external returns (bytes32 intentHash)',
        'function fulfillIntent(bytes calldata _paymentProof, bytes32 _intentHash) external',
    ];
    function asEthersSigner(signer) {
        const s = signer;
        if (s?.provider)
            return s;
        throw new Error('Signer must have a provider. Use ethers BrowserProvider.getSigner() for ZKP2P flows.');
    }
    /**
     * Call signalIntent on Escrow. Returns intentHash.
     */
    async function callSignalIntent(signer, verified) {
        const ethersSigner = asEthersSigner(signer);
        const contract = new ethers.ethers.Contract(ESCROW_ADDRESS, ESCROW_ABI, ethersSigner);
        const { intentData } = verified;
        const intentHash = await contract.signalIntent(BigInt(intentData.depositId), BigInt(intentData.tokenAmount), intentData.recipientAddress, intentData.verifierAddress, intentData.currencyCodeHash, intentData.gatingServiceSignature);
        return intentHash;
    }
    /**
     * Call fulfillIntent on Escrow. Returns tx hash.
     */
    async function callFulfillIntent(signer, proofBytes, intentHash) {
        const ethersSigner = asEthersSigner(signer);
        const contract = new ethers.ethers.Contract(ESCROW_ADDRESS, ESCROW_ABI, ethersSigner);
        const tx = await contract.fulfillIntent(proofBytes, intentHash);
        const receipt = await tx.wait();
        return { hash: receipt.hash };
    }

    /**
     * signalIntent, fulfillIntent — Escrow contract calls
     */
    /**
     * Signal intent on ZKP2P Escrow. Returns intentHash.
     * Signer must have provider (e.g. ethers provider.getSigner()).
     */
    async function signalIntent(signer, verifiedIntent) {
        return callSignalIntent(signer, verifiedIntent);
    }
    /**
     * Fulfill intent on ZKP2P Escrow. Returns tx receipt with hash.
     */
    async function fulfillIntent(signer, proofBytes, intentHash) {
        return callFulfillIntent(signer, proofBytes, intentHash);
    }

    /**
     * PeerAuth extension adapter — proof generation for ZKP2P
     */
    /**
     * Encode proof as bytes for fulfillIntent (JSON -> TextEncoder -> 0x hex)
     */
    function encodeProofAsBytes(proof) {
        const proofString = JSON.stringify(proof);
        const encoder = new TextEncoder();
        const proofBytes = encoder.encode(proofString);
        return ('0x' +
            Array.from(proofBytes)
                .map((b) => b.toString(16).padStart(2, '0'))
                .join(''));
    }
    /**
     * Generate and encode proof via PeerAuth extension.
     * Requires window.zktls (PeerAuth extension installed).
     */
    async function generateAndEncodeProof$1(intentHash, platform, originalIndex = 0) {
        if (typeof window === 'undefined') {
            throw new Error(`ZKP2P proof generation requires the PeerAuth browser extension. Install from: ${ZKP2P_EXTENSION_INSTALL_URL}`);
        }
        const zktls = window.zktls;
        if (!zktls || typeof zktls.requestConnection !== 'function') {
            throw new Error(`ZKP2P path requires PeerAuth browser extension. Install from Chrome Web Store: ${ZKP2P_EXTENSION_INSTALL_URL}`);
        }
        const connected = await zktls.requestConnection();
        if (!connected) {
            throw new Error('PeerAuth connection was not approved');
        }
        const { proofId } = await zktls.generateProof({
            intentHash,
            originalIndex,
            platform: platform.toLowerCase(),
        });
        const { notaryRequest } = await zktls.fetchProofById(proofId);
        const proofBytes = encodeProofAsBytes(notaryRequest);
        return proofBytes;
    }

    /**
     * generateAndEncodeProof — PeerAuth proof generation
     */
    /**
     * Generate and encode payment proof via PeerAuth extension.
     * Requires PeerAuth extension (window.zktls). Use getZkp2pStatus() to check availability.
     */
    async function generateAndEncodeProof(intentHash, platform, originalIndex = 0) {
        return generateAndEncodeProof$1(intentHash, platform, originalIndex);
    }

    /**
     * Default storage adapter — localStorage
     */
    const KEY_PREFIX = 'zkp2p-donate:v1:';
    function storageKey(accountId) {
        return `${KEY_PREFIX}${accountId}`;
    }
    const defaultStorage = {
        async get(key) {
            if (typeof localStorage === 'undefined')
                return null;
            const raw = localStorage.getItem(key);
            if (!raw)
                return null;
            try {
                const parsed = JSON.parse(raw);
                if (typeof parsed.lastDonationAt !== 'string')
                    return null;
                return parsed;
            }
            catch {
                return null;
            }
        },
        async set(key, value) {
            if (typeof localStorage === 'undefined') {
                throw new Error('localStorage is not available');
            }
            localStorage.setItem(key, JSON.stringify(value));
        },
    };

    /**
     * recordDonation, getDonationStatus — donation recording and support status
     */
    /**
     * Record a donation for an account. Call after successful crypto or ZKP2P donation.
     */
    async function recordDonation(accountId, data, options) {
        const storage = options?.storage ?? defaultStorage;
        const key = storageKey(accountId);
        const record = {
            lastDonationAt: new Date().toISOString(),
            ...(data.txHash && { txHash: data.txHash }),
            ...(data.amount && { amount: data.amount }),
            ...(data.chainId != null && { chainId: data.chainId }),
        };
        await storage.set(key, record);
    }
    /**
     * Get donation status for an account. Returns valid if last donation is within maxAgeMs.
     */
    async function getDonationStatus(accountId, options) {
        const { maxAgeMs, storage = defaultStorage } = options;
        const key = storageKey(accountId);
        const record = await storage.get(key);
        if (!record) {
            return { valid: false };
        }
        const lastAt = new Date(record.lastDonationAt).getTime();
        const now = Date.now();
        const valid = lastAt + maxAgeMs > now;
        const expiredAt = valid ? undefined : new Date(lastAt + maxAgeMs).toISOString();
        return {
            valid,
            lastDonationAt: record.lastDonationAt,
            expiredAt,
        };
    }

    /**
     * 402 Payment Required — versioned contract (v1)
     * Frozen types for cross-project compatibility. Additive changes only.
     */
    /** Validate 402 body has required fields */
    function isPaymentRequiredBody(body) {
        if (!body || typeof body !== 'object')
            return false;
        const o = body;
        return (o.paymentRequired === true &&
            typeof o.recipient === 'string' &&
            typeof o.chainId === 'number');
    }

    /**
     * handle402 — client-side 402 Payment Required flow
     */
    /**
     * Handle 402 Payment Required. Pays via crypto or ZKP2P, returns PaymentProof for retry.
     */
    async function handle402(body, options) {
        if (!isPaymentRequiredBody(body)) {
            throw new Error('Invalid 402 body: missing paymentRequired, recipient, or chainId');
        }
        const { recipient, chainId, amountWei, zkp2p } = body;
        const resolvedRecipient = await resolveAddress(recipient, options.provider);
        const chain = chainId ?? BASE_CHAIN_ID;
        if (options.useZkp2p && zkp2p?.enabled && zkp2p.verifyUrl) {
            return runZkp2pPath(options.signer, resolvedRecipient, body, options);
        }
        return runCryptoPath(options.signer, resolvedRecipient, chain, amountWei);
    }
    async function runCryptoPath(signer, recipient, chainId, amountWei) {
        const value = amountWei ? BigInt(amountWei) : 0n;
        const tx = await signer.sendTransaction({
            to: recipient,
            value,
        });
        return {
            type: 'crypto',
            chainId,
            txHash: tx.hash,
            recipient,
            ...(amountWei && { amount: amountWei }),
        };
    }
    async function runZkp2pPath(signer, recipient, body, options) {
        const verifyUrl = body.zkp2p?.verifyUrl ?? options.verifyUrl;
        if (!verifyUrl) {
            throw new Error('ZKP2P path requires verifyUrl in 402 body or options');
        }
        const userAddress = await signer.getAddress();
        const amountUsd = body.amountFormatted
            ? parseFloat(body.amountFormatted.replace(/[^0-9.]/g, ''))
            : 5;
        const quote = await getQuote({
            recipient,
            amountUsd,
            userAddress,
            provider: options.provider,
            chainId: body.chainId,
        });
        const verifyRes = await fetch(verifyUrl, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(quote.intent),
        });
        if (!verifyRes.ok) {
            throw new Error(`Verify intent failed: ${verifyRes.status}`);
        }
        const json = (await verifyRes.json());
        const verified = json && typeof json === 'object' && 'responseObject' in json && json.responseObject
            ? json.responseObject
            : json;
        const intentHash = await signalIntent(signer, verified);
        const proofBytes = await generateAndEncodeProof(intentHash, quote.platform);
        const { hash } = await fulfillIntent(signer, proofBytes, intentHash);
        return {
            type: 'zkp2p',
            chainId: body.chainId,
            txHash: hash,
            recipient,
        };
    }

    /**
     * Orchestration helpers — runZkp2pDonation, completeZkp2pDonation
     */
    /**
     * Run quote -> verify (via callback) -> signalIntent.
     * Returns { intentHash, payeeAddress, platform } so app can show "Pay $X to @user".
     * Then app calls completeZkp2pDonation after user pays.
     */
    async function runZkp2pDonation(options) {
        const { signer, recipient, amountUsd, platform, getVerifiedIntent, provider, apiBaseUrl } = options;
        const userAddress = await signer.getAddress();
        const quote = await getQuote({
            recipient,
            amountUsd,
            userAddress,
            platform,
            provider,
            apiBaseUrl,
        });
        const verified = await getVerifiedIntent(quote.intent);
        const intentHash = await signalIntent(signer, verified);
        return {
            intentHash,
            payeeAddress: quote.payeeAddress,
            platform: quote.platform,
        };
    }
    /**
     * Complete ZKP2P donation: generate proof -> fulfillIntent.
     * Call after user has made the Venmo/Cash App payment.
     */
    async function completeZkp2pDonation(signer, intentHash, platform) {
        const proofBytes = await generateAndEncodeProof(intentHash, platform);
        return fulfillIntent(signer, proofBytes, intentHash);
    }

    /**
     * Capability detection — wallet and ZKP2P extension availability
     */
    /** Check if a wallet (e.g. MetaMask) is available via window.ethereum */
    function getWalletStatus() {
        if (typeof window === 'undefined') {
            return { available: false };
        }
        return { available: typeof window.ethereum !== 'undefined' };
    }
    /**
     * Check if the Peer extension is available for the redirect onramp (Venmo/Cash App).
     * Uses the same globals the onramp flow uses: window.peer (Peer SDK) or window.zktls.
     * When only peer is present, openDonation/openRedirectOnramp work; proof generation requires zktls.
     */
    function getZkp2pStatus() {
        if (typeof window === 'undefined') {
            return { available: false, needsInstall: true, proofAvailable: false };
        }
        const peer = typeof window.peer !== 'undefined';
        const zktls = typeof window.zktls !== 'undefined';
        const available = peer || zktls;
        return { available, needsInstall: !available, proofAvailable: zktls };
    }
    /**
     * Wait for the Peer extension to become available (e.g. after async injection).
     * Listens for zktls#initialized and polls getZkp2pStatus(). Resolves when available or after timeoutMs.
     * Use before deciding to show "install extension" so UIs don't flash "not installed" when the extension is installed but not yet injected.
     */
    function whenExtensionAvailable(options = {}) {
        const { timeoutMs = 3000, pollIntervalMs = 100 } = options;
        if (typeof window === 'undefined') {
            return Promise.resolve();
        }
        const status = getZkp2pStatus();
        if (status.available) {
            return Promise.resolve();
        }
        return new Promise((resolve) => {
            const deadline = Date.now() + timeoutMs;
            const check = () => {
                if (getZkp2pStatus().available) {
                    cleanup();
                    resolve();
                    return;
                }
                if (Date.now() >= deadline) {
                    cleanup();
                    resolve();
                    return;
                }
            };
            const onInitialized = () => {
                check();
            };
            const cleanup = () => {
                window.removeEventListener('zktls#initialized', onInitialized);
                clearInterval(intervalId);
            };
            window.addEventListener('zktls#initialized', onInitialized);
            const intervalId = setInterval(check, pollIntervalMs);
        });
    }

    if (ethers && ethers.AbiCoder) { ethers.AbiCoder.defaultAbiCoder(); }

    // src/extension.ts
    var PEER_EXTENSION_CHROME_URL = "https://chromewebstore.google.com/detail/peerauth-authenticate-and/ijpgccednehjpeclfcllnjjcmiohdjih";
    var resolveWindow = (options) => {
      if (options?.window) {
        return options.window;
      }
      if (typeof window === "undefined") {
        return void 0;
      }
      return window;
    };
    var requirePeer = (options) => {
      const resolvedWindow = resolveWindow(options);
      if (!resolvedWindow) {
        throw new Error("Peer extension SDK requires a browser window.");
      }
      if (!resolvedWindow.peer) {
        throw new Error("Peer extension not available. Install or enable the Peer extension.");
      }
      return resolvedWindow.peer;
    };
    var isPeerExtensionAvailable = (options) => {
      const resolvedWindow = resolveWindow(options);
      return Boolean(resolvedWindow?.peer);
    };
    var openPeerExtensionInstallPage = (options) => {
      const resolvedWindow = resolveWindow(options);
      if (!resolvedWindow) {
        throw new Error("Peer extension SDK requires a browser window.");
      }
      resolvedWindow.open(PEER_EXTENSION_CHROME_URL, "_blank", "noopener,noreferrer");
    };
    var getPeerExtensionState = async (options) => {
      if (!isPeerExtensionAvailable(options)) {
        return "needs_install";
      }
      try {
        const status = await requirePeer(options).checkConnectionStatus();
        return status === "connected" ? "ready" : "needs_connection";
      } catch (error) {
        return "needs_connection";
      }
    };
    var fiatAmountRegex = /^-?\d*(\.\d{0,6})?$/;
    var usdcAmountRegex = /^\d+$/;
    var assertObjectInput = (params) => {
      if (params === void 0) {
        return {};
      }
      if (params === null || typeof params !== "object" || Array.isArray(params)) {
        throw new Error("Peer extension onramp expects an object of query params.");
      }
      return params;
    };
    var normalizeOptionalString = (value, label) => {
      if (value === void 0) {
        return void 0;
      }
      if (typeof value !== "string") {
        throw new Error(`Peer extension onramp ${label} must be a non-empty string.`);
      }
      const trimmed = value.trim();
      if (!trimmed) {
        throw new Error(`Peer extension onramp ${label} must be a non-empty string.`);
      }
      return trimmed;
    };
    var normalizeOptionalUrl = (value, label) => {
      const trimmed = normalizeOptionalString(value, label);
      if (trimmed === void 0) {
        return void 0;
      }
      let parsed;
      try {
        parsed = new URL(trimmed);
      } catch (error) {
        throw new Error(`Peer extension onramp ${label} must be a valid URL.`);
      }
      if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
        throw new Error(`Peer extension onramp ${label} must use http or https.`);
      }
      return trimmed;
    };
    var normalizeFiatAmount = (value) => {
      if (value === void 0) {
        return void 0;
      }
      const normalized = typeof value === "number" ? String(value) : value;
      if (typeof normalized !== "string") {
        throw new Error("Peer extension onramp inputAmount must be a string or number.");
      }
      const trimmed = normalized.trim();
      if (!trimmed) {
        throw new Error("Peer extension onramp inputAmount must be a non-empty value.");
      }
      if (!fiatAmountRegex.test(trimmed)) {
        throw new Error(
          "Peer extension onramp inputAmount must be a non-negative number with up to 6 decimals."
        );
      }
      if (Number.isNaN(Number(trimmed)) || Number(trimmed) < 0) {
        throw new Error(
          "Peer extension onramp inputAmount must be a non-negative number with up to 6 decimals."
        );
      }
      return trimmed;
    };
    var normalizeUsdcAmount = (value) => {
      if (value === void 0) {
        return void 0;
      }
      if (typeof value === "bigint") {
        if (value < 0n) {
          throw new Error("Peer extension onramp amountUsdc must be a non-negative integer.");
        }
        return value.toString();
      }
      if (typeof value === "number") {
        if (!Number.isFinite(value) || value < 0 || !Number.isInteger(value)) {
          throw new Error("Peer extension onramp amountUsdc must be a non-negative integer.");
        }
        return String(value);
      }
      if (typeof value !== "string") {
        throw new Error("Peer extension onramp amountUsdc must be a string, number, or bigint.");
      }
      const trimmed = value.trim();
      if (!trimmed) {
        throw new Error("Peer extension onramp amountUsdc must be a non-empty value.");
      }
      if (!usdcAmountRegex.test(trimmed)) {
        throw new Error("Peer extension onramp amountUsdc must be a non-negative integer.");
      }
      return trimmed;
    };
    var intentHashRegex = /^0x[a-fA-F0-9]{64}$/;
    var normalizeIntentHash = (value) => {
      if (value === void 0) {
        return void 0;
      }
      if (typeof value !== "string") {
        throw new Error("Peer extension onramp intentHash must be a string.");
      }
      const trimmed = value.trim();
      if (!trimmed) {
        throw new Error("Peer extension onramp intentHash must be a non-empty string.");
      }
      if (!intentHashRegex.test(trimmed)) {
        throw new Error(
          "Peer extension onramp intentHash must be a valid bytes32 hex string (0x + 64 hex characters)."
        );
      }
      return trimmed.toLowerCase();
    };
    var buildOnrampQueryString = (params) => {
      const validated = assertObjectInput(params);
      const searchParams = new URLSearchParams();
      const setParam = (key, value) => {
        if (value !== void 0) {
          searchParams.set(key, value);
        }
      };
      setParam("referrer", normalizeOptionalString(validated.referrer, "referrer"));
      setParam("referrerLogo", normalizeOptionalUrl(validated.referrerLogo, "referrerLogo"));
      setParam("inputCurrency", normalizeOptionalString(validated.inputCurrency, "inputCurrency"));
      setParam("inputAmount", normalizeFiatAmount(validated.inputAmount));
      setParam(
        "paymentPlatform",
        normalizeOptionalString(validated.paymentPlatform, "paymentPlatform")
      );
      setParam("toToken", normalizeOptionalString(validated.toToken, "toToken"));
      setParam("amountUsdc", normalizeUsdcAmount(validated.amountUsdc));
      setParam(
        "recipientAddress",
        normalizeOptionalString(validated.recipientAddress, "recipientAddress")
      );
      setParam("callbackUrl", normalizeOptionalUrl(validated.callbackUrl, "callbackUrl"));
      setParam("intentHash", normalizeIntentHash(validated.intentHash));
      return searchParams.toString();
    };
    var createPeerExtensionSdk = (options = {}) => ({
      isAvailable: () => isPeerExtensionAvailable(options),
      requestConnection: () => requirePeer(options).requestConnection(),
      checkConnectionStatus: () => requirePeer(options).checkConnectionStatus(),
      openSidebar: (route) => requirePeer(options).openSidebar(route),
      onramp: (params) => requirePeer(options).onramp(buildOnrampQueryString(params)),
      onProofComplete: (callback) => requirePeer(options).onProofComplete(callback),
      getVersion: () => requirePeer(options).getVersion(),
      openInstallPage: () => openPeerExtensionInstallPage(options),
      getState: () => getPeerExtensionState(options)
    });
    var peerExtensionSdk = createPeerExtensionSdk();

    /**
     * Redirect flow — open Peer extension onramp with p2pago defaults
     */
    /** Base USDC on Base (chainId:tokenAddress) */
    const BASE_USDC = '8453:0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913';
    /**
     * Open the Peer extension onramp (redirect flow).
     * Gasless; extension handles submission. Requires Peer extension installed.
     *
     * @param options — Override defaults. referrer defaults to "p2pago".
     */
    function openRedirectOnramp(options = {}) {
        const { recipientAddress = P2PAGO_DEFAULT_RECIPIENT, amountUsd, inputAmount, toToken = BASE_USDC, paymentPlatform, referrer = P2PAGO_DEFAULT_REFERRER, referrerLogo, callbackUrl, inputCurrency = 'USD', } = options;
        const resolvedInputAmount = inputAmount != null ? String(inputAmount) : amountUsd != null ? String(amountUsd) : undefined;
        peerExtensionSdk.onramp({
            referrer,
            ...(referrerLogo && { referrerLogo }),
            recipientAddress,
            inputCurrency,
            ...(resolvedInputAmount && { inputAmount: resolvedInputAmount }),
            ...(paymentPlatform && { paymentPlatform }),
            toToken,
            ...(callbackUrl && { callbackUrl }),
        });
    }
    /**
     * Returns true if amount is below the small-donation warning threshold.
     */
    function isSmallDonation(amountUsd) {
        const n = typeof amountUsd === 'string' ? parseFloat(amountUsd) : amountUsd;
        return !Number.isNaN(n) && n < MIN_DONATION_WARNING_USD;
    }
    /**
     * Open donation flow (redirect). Checks extension, optionally warns on small amount.
     * Throws if extension not installed (unless openInstallPageIfMissing).
     */
    function openDonation(options = {}) {
        const { onSmallAmountWarning, openInstallPageIfMissing = false, ...onrampOpts } = options;
        const { available } = getZkp2pStatus();
        if (!available) {
            if (openInstallPageIfMissing && typeof window !== 'undefined') {
                window.open(ZKP2P_EXTENSION_INSTALL_URL, '_blank', 'noopener,noreferrer');
                return;
            }
            throw new Error(`Peer extension required for ZKP2P donations. Install from: ${ZKP2P_EXTENSION_INSTALL_URL}`);
        }
        const amount = onrampOpts.inputAmount != null
            ? (typeof onrampOpts.inputAmount === 'string'
                ? parseFloat(onrampOpts.inputAmount)
                : onrampOpts.inputAmount)
            : onrampOpts.amountUsd != null
                ? (typeof onrampOpts.amountUsd === 'string' ? parseFloat(onrampOpts.amountUsd) : onrampOpts.amountUsd)
                : undefined;
        if (amount != null && isSmallDonation(amount) && onSmallAmountWarning) {
            const msg = `Donations under $${MIN_DONATION_WARNING_USD} may take longer to process.`;
            const proceed = onSmallAmountWarning(msg);
            if (proceed === false)
                return;
        }
        openRedirectOnramp(onrampOpts);
    }

    /** SDK version for debugging and proof metadata */
    const SDK_VERSION = '0.1.0';

    /**
     * Supported chains and token metadata for payments and verification.
     * Apps can subset or extend; SDK uses this as the canonical source for RPC and token addresses.
     */
    /** Native ETH sentinel (zero address). Use for native transfer verification. */
    const NATIVE_TOKEN_ADDRESS = '0x0000000000000000000000000000000000000000';
    /** ERC20 Transfer event topic (Transfer(address,address,uint256)) */
    const ERC20_TRANSFER_TOPIC = '0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef';
    const BASE_RPC = 'https://mainnet.base.org';
    const ETHEREUM_RPC = 'https://ethereum.publicnode.com';
    const POLYGON_RPC = 'https://polygon-rpc.com';
    const ARBITRUM_RPC = 'https://arb1.arbitrum.io/rpc';
    const OPTIMISM_RPC = 'https://mainnet.optimism.io';
    /** USDC addresses (Circle canonical mainnet). */
    const USDC = {
        1: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
        8453: '0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913',
        137: '0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359',
        42161: '0xaf88d065e77c8cC2239327C5EDb3A432268e5831',
        10: '0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85',
    };
    /** USDT addresses (Tether, common mainnet). */
    const USDT = {
        1: '0xdAC17F958D2ee523a2206206994597C13D831ec7',
        8453: '0xfde4C96c8593536E31F229EA8f37b2ADa2699bb2',
        137: '0xc2132D05D31c914a87C6611C10748AEb04B58e8F',
        42161: '0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9',
        10: '0x94b008aA00579c1307B0EF2c499aD98a8ce58e58',
    };
    function buildChains() {
        const chains = {};
        const entries = [
            { chainId: 1, name: 'Ethereum', rpcUrl: ETHEREUM_RPC, usdc: USDC[1], usdt: USDT[1] },
            { chainId: 8453, name: 'Base', rpcUrl: BASE_RPC, usdc: USDC[8453], usdt: USDT[8453] },
            { chainId: 137, name: 'Polygon', rpcUrl: POLYGON_RPC, usdc: USDC[137], usdt: USDT[137] },
            { chainId: 42161, name: 'Arbitrum One', rpcUrl: ARBITRUM_RPC, usdc: USDC[42161], usdt: USDT[42161] },
            { chainId: 10, name: 'OP Mainnet', rpcUrl: OPTIMISM_RPC, usdc: USDC[10], usdt: USDT[10] },
        ];
        for (const { chainId, name, rpcUrl, usdc, usdt } of entries) {
            chains[chainId] = {
                name,
                chainId,
                rpcUrl,
                tokens: [
                    { address: NATIVE_TOKEN_ADDRESS, symbol: 'ETH', decimals: 18 },
                    { address: usdc, symbol: 'USDC', decimals: 6 },
                    { address: usdt, symbol: 'USDT', decimals: 6 },
                ],
            };
        }
        return chains;
    }
    /** Supported chains with name, rpcUrl, and default tokens (ETH, USDC, USDT). Apps can subset or extend. */
    const SUPPORTED_CHAINS = buildChains();
    /**
     * Return the supported chain config. Use for "pay with wallet" UI (display names, USDC address, decimals)
     * and for verification (RPC URL). Apps can pass a custom chain map to verifyPaymentTx if needed.
     */
    function getSupportedChains() {
        return SUPPORTED_CHAINS;
    }

    /**
     * On-chain verification of a direct payment tx (native or ERC20 to recipient).
     */
    /**
     * Verify that a tx succeeded and that value (native or ERC20) reached the recipient.
     * Uses SDK chain config for RPC unless rpcUrl is passed. Returns true only if the tx
     * succeeded and the recipient received the payment.
     */
    async function verifyPaymentTx(params) {
        const { txHash, chainId, recipientAddress, tokenAddress, rpcUrl } = params;
        const chains = getSupportedChains();
        const chain = chains[chainId];
        const url = rpcUrl ?? chain?.rpcUrl;
        if (!url) {
            throw new Error(`No RPC URL for chainId ${chainId}. Pass rpcUrl or use a supported chain.`);
        }
        const receipt = await rpcCall(url, 'eth_getTransactionReceipt', [txHash]);
        if (!receipt || !receipt.status) {
            return false;
        }
        if (receipt.status !== '0x1') {
            return false;
        }
        if (tokenAddress) {
            const recipientTopic = addressToTopic(recipientAddress);
            const hasTransferToRecipient = (receipt.logs ?? []).some((log) => log.address.toLowerCase() === tokenAddress.toLowerCase() &&
                log.topics?.[0] === ERC20_TRANSFER_TOPIC &&
                log.topics?.[2] === recipientTopic);
            return hasTransferToRecipient;
        }
        const tx = await rpcCall(url, 'eth_getTransactionByHash', [txHash]);
        if (!tx?.to || !tx.value) {
            return false;
        }
        const valueWei = BigInt(tx.value);
        if (valueWei === 0n) {
            return false;
        }
        return tx.to.toLowerCase() === recipientAddress.toLowerCase();
    }
    function addressToTopic(address) {
        const hex = address.startsWith('0x') ? address.slice(2).toLowerCase() : address.toLowerCase();
        if (hex.length !== 40) {
            throw new Error(`Invalid address: ${address}`);
        }
        return '0x' + hex.padStart(64, '0');
    }
    async function rpcCall(url, method, params) {
        const res = await fetch(url, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                jsonrpc: '2.0',
                id: 1,
                method,
                params,
            }),
        });
        if (!res.ok) {
            throw new Error(`RPC request failed: ${res.status}`);
        }
        const json = (await res.json());
        if (json.error) {
            throw new Error(json.error.message ?? 'RPC error');
        }
        return json.result ?? null;
    }

    exports.DEFAULT_MAINNET_RPC_URL = DEFAULT_MAINNET_RPC_URL;
    exports.ERC20_TRANSFER_TOPIC = ERC20_TRANSFER_TOPIC;
    exports.GAS_COST_MAX_FRACTION = GAS_COST_MAX_FRACTION;
    exports.MIN_DONATION_WARNING_USD = MIN_DONATION_WARNING_USD;
    exports.NATIVE_TOKEN_ADDRESS = NATIVE_TOKEN_ADDRESS;
    exports.P2PAGO_DEFAULT_RECIPIENT = P2PAGO_DEFAULT_RECIPIENT;
    exports.P2PAGO_DEFAULT_REFERRER = P2PAGO_DEFAULT_REFERRER;
    exports.P2PAGO_FEE_MIN_USD = P2PAGO_FEE_MIN_USD;
    exports.P2PAGO_FEE_PERCENT = P2PAGO_FEE_PERCENT;
    exports.SDK_VERSION = SDK_VERSION;
    exports.SUPPORTED_CHAINS = SUPPORTED_CHAINS;
    exports.ZKP2P_EXTENSION_INSTALL_URL = ZKP2P_EXTENSION_INSTALL_URL;
    exports.completeZkp2pDonation = completeZkp2pDonation;
    exports.fulfillIntent = fulfillIntent;
    exports.generateAndEncodeProof = generateAndEncodeProof;
    exports.getDonationStatus = getDonationStatus;
    exports.getQuote = getQuote;
    exports.getSupportedChains = getSupportedChains;
    exports.getWalletStatus = getWalletStatus;
    exports.getZkp2pStatus = getZkp2pStatus;
    exports.handle402 = handle402;
    exports.isSmallDonation = isSmallDonation;
    exports.openDonation = openDonation;
    exports.openRedirectOnramp = openRedirectOnramp;
    exports.recordDonation = recordDonation;
    exports.resolveRecipient = resolveRecipient;
    exports.runZkp2pDonation = runZkp2pDonation;
    exports.signalIntent = signalIntent;
    exports.verifyIntent = verifyIntent;
    exports.verifyPaymentTx = verifyPaymentTx;
    exports.whenExtensionAvailable = whenExtensionAvailable;

}));
