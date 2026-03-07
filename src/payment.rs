#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = p2pagoPaymentValid)]
    fn p2pago_payment_valid() -> bool;
    #[wasm_bindgen(js_name = p2pagoPromptPayment)]
    fn p2pago_prompt_payment();
    #[wasm_bindgen(js_name = p2pagoOpenPaymentModal)]
    fn p2pago_open_payment_modal();
}

pub fn support_valid() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        return p2pago_payment_valid();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        true
    }
}

pub fn prompt_support() {
    #[cfg(target_arch = "wasm32")]
    {
        p2pago_open_payment_modal();
    }
}

pub fn open_payment_modal() {
    #[cfg(target_arch = "wasm32")]
    {
        p2pago_open_payment_modal();
    }
}
