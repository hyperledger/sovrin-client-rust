use std::collections::HashMap;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

use hex;

use serde_json;

use indy_api_types::errors::prelude::*;
use crate::services::crypto::CryptoService;
use crate::services::ledger::LedgerService;
use crate::services::payments::{PaymentsMethodCBs, PaymentsService, RequesterInfo, Fees};
use indy_wallet::{RecordOptions, WalletService};
use indy_api_types::WalletHandle;
use crate::domain::ledger::auth_rule::AuthRule;
use crate::domain::crypto::did::DidValue;

use crate::commands::BoxedCallbackStringStringSend;

pub enum PaymentsCommand {
    RegisterMethod(
        String, //type
        PaymentsMethodCBs, //method callbacks
        Box<dyn Fn(IndyResult<()>) + Send>),
    CreateAddress(
        WalletHandle,
        String, //type
        String, //config
        Box<dyn Fn(IndyResult<String>) + Send>),
    ListAddresses(
        WalletHandle,
        Box<dyn Fn(IndyResult<String>) + Send>),
    AddRequestFees(
        WalletHandle,
        Option<DidValue>, //submitter did
        String, //req
        String, //inputs
        String, //outputs
        Option<String>, //extra
        BoxedCallbackStringStringSend),
    ParseResponseWithFees(
        String, //type
        String, //response
        Box<dyn Fn(IndyResult<String>) + Send>),
    BuildGetPaymentSourcesRequest(
        WalletHandle,
        Option<DidValue>, //submitter did
        String, //payment address
        Option<i64>, //from
        BoxedCallbackStringStringSend),
    ParseGetPaymentSourcesResponse(
        String, //type
        String, //response
        Box<dyn Fn(IndyResult<(String, i64)>) + Send>),
    BuildPaymentReq(
        WalletHandle,
        Option<DidValue>, //submitter did
        String, //inputs
        String, //outputs
        Option<String>, //extra
        BoxedCallbackStringStringSend),
    ParsePaymentResponse(
        String, //payment_method
        String, //response
        Box<dyn Fn(IndyResult<String>) + Send>),
    AppendTxnAuthorAgreementAcceptanceToExtra(
        Option<String>, // extra json
        Option<String>, // text
        Option<String>, // version
        Option<String>, // hash
        String, // acceptance mechanism type
        u64, // time of acceptance
        Box<dyn Fn(IndyResult<String>) + Send>),
    BuildMintReq(
        WalletHandle,
        Option<DidValue>, //submitter did
        String, //outputs
        Option<String>, //extra
        BoxedCallbackStringStringSend),
    BuildSetTxnFeesReq(
        WalletHandle,
        Option<DidValue>, //submitter did
        String, //method
        String, //fees
        Box<dyn Fn(IndyResult<String>) + Send>),
    BuildGetTxnFeesReq(
        WalletHandle,
        Option<DidValue>, //submitter did
        String, //method
        Box<dyn Fn(IndyResult<String>) + Send>),
    ParseGetTxnFeesResponse(
        String, //method
        String, //response
        Box<dyn Fn(IndyResult<String>) + Send>),
    BuildVerifyPaymentReq(
        WalletHandle,
        Option<DidValue>, //submitter_did
        String, //receipt
        BoxedCallbackStringStringSend),
    ParseVerifyPaymentResponse(
        String, //payment_method
        String, //resp_json
        Box<dyn Fn(IndyResult<String>) + Send>),
    GetRequestInfo(
        String, // get auth rule response json
        RequesterInfo, //requester info
        Fees, //fees
        Box<dyn Fn(IndyResult<String>) + Send>),
    SignWithAddressReq(
        WalletHandle,
        String, //address
        Vec<u8>, //message
        Box<dyn Fn(IndyResult<Vec<u8>>) + Send>),
    VerifyWithAddressReq(
        String, //address
        Vec<u8>, //message
        Vec<u8>, //signature
        Box<dyn Fn(IndyResult<bool>) + Send>),
}

pub struct PaymentsCommandExecutor {
    payments_service: Rc<PaymentsService>,
    wallet_service: Rc<WalletService>,
    crypto_service: Rc<CryptoService>,
    ledger_service: Rc<LedgerService>,
}

impl PaymentsCommandExecutor {
    pub fn new(payments_service: Rc<PaymentsService>, wallet_service: Rc<WalletService>, crypto_service: Rc<CryptoService>, ledger_service: Rc<LedgerService>) -> PaymentsCommandExecutor {
        PaymentsCommandExecutor {
            payments_service,
            wallet_service,
            crypto_service,
            ledger_service,
        }
    }

    pub async fn execute(&self, command: PaymentsCommand) {
        match command {
            PaymentsCommand::RegisterMethod(type_, method_cbs, cb) => {
                debug!(target: "payments_command_executor", "RegisterMethod command received");
                cb(self.register_method(&type_, method_cbs));
            }
            PaymentsCommand::CreateAddress(wallet_handle, type_, config, cb) => {
                debug!(target: "payments_command_executor", "CreateAddress command received");
                self.create_address(wallet_handle, &type_, &config, cb).await;
            }
            PaymentsCommand::ListAddresses(wallet_handle, cb) => {
                debug!(target: "payments_command_executor", "ListAddresses command received");
                self.list_addresses(wallet_handle, cb);
            }
            PaymentsCommand::AddRequestFees(wallet_handle, submitter_did, req, inputs, outputs, extra, cb) => {
                debug!(target: "payments_command_executor", "AddRequestFees command received");
                self.add_request_fees(wallet_handle, submitter_did.as_ref(), &req, &inputs, &outputs, extra.as_ref().map(String::as_str), cb).await;
            }
            PaymentsCommand::ParseResponseWithFees(type_, response, cb) => {
                debug!(target: "payments_command_executor", "ParseResponseWithFees command received");
                self.parse_response_with_fees(&type_, &response, cb).await;
            }
            PaymentsCommand::BuildGetPaymentSourcesRequest(wallet_handle, submitter_did, payment_address, from, cb) => {
                debug!(target: "payments_command_executor", "BuildGetPaymentSourcesRequest command received");
                self.build_get_payment_sources_request(wallet_handle, submitter_did.as_ref(), &payment_address, from, cb).await;
            }
            PaymentsCommand::ParseGetPaymentSourcesResponse(type_, response, cb) => {
                debug!(target: "payments_command_executor", "ParseGetPaymentSourcesResponse command received");
                self.parse_get_payment_sources_response(&type_, &response, cb).await;
            }
            PaymentsCommand::BuildPaymentReq(wallet_handle, submitter_did, inputs, outputs, extra, cb) => {
                debug!(target: "payments_command_executor", "BuildPaymentReq command received");
                self.build_payment_req(wallet_handle, submitter_did.as_ref(), &inputs, &outputs, extra.as_ref().map(String::as_str), cb).await;
            }
            PaymentsCommand::ParsePaymentResponse(payment_method, response, cb) => {
                debug!(target: "payments_command_executor", "ParsePaymentResponse command received");
                self.parse_payment_response(&payment_method, &response, cb).await;
            }
            PaymentsCommand::AppendTxnAuthorAgreementAcceptanceToExtra(extra, text, version, taa_digest, mechanism, time, cb) => {
                debug!(target: "payments_command_executor", "AppendTxnAuthorAgreementAcceptanceToExtra command received");
                cb(self.append_txn_author_agreement_acceptance_to_extra(extra.as_ref().map(String::as_str),
                                                                        text.as_ref().map(String::as_str),
                                                                        version.as_ref().map(String::as_str),
                                                                        taa_digest.as_ref().map(String::as_str),
                                                                        &mechanism,
                                                                        time));
            }
            PaymentsCommand::BuildMintReq(wallet_handle, submitter_did, outputs, extra, cb) => {
                debug!(target: "payments_command_executor", "BuildMintReq command received");
                self.build_mint_req(wallet_handle, submitter_did.as_ref(), &outputs, extra.as_ref().map(String::as_str), cb).await;
            }
            PaymentsCommand::BuildSetTxnFeesReq(wallet_handle, submitter_did, type_, fees, cb) => {
                debug!(target: "payments_command_executor", "BuildSetTxnFeesReq command received");
                self.build_set_txn_fees_req(wallet_handle, submitter_did.as_ref(), &type_, &fees, cb).await;
            }
            PaymentsCommand::BuildGetTxnFeesReq(wallet_handle, submitter_did, type_, cb) => {
                debug!(target: "payments_command_executor", "BuildGetTxnFeesReq command received");
                self.build_get_txn_fees_req(wallet_handle, submitter_did.as_ref(), &type_, cb).await;
            }
            PaymentsCommand::ParseGetTxnFeesResponse(type_, response, cb) => {
                debug!(target: "payments_command_executor", "ParseGetTxnFeesResponse command received");
                self.parse_get_txn_fees_response(&type_, &response, cb).await;
            }
            PaymentsCommand::BuildVerifyPaymentReq(wallet_handle, submitter_did, receipt, cb) => {
                debug!(target: "payments_command_executor", "BuildVerifyPaymentReq command received");
                self.build_verify_payment_request(wallet_handle, submitter_did.as_ref(), &receipt, cb).await;
            }
            PaymentsCommand::ParseVerifyPaymentResponse(payment_method, resp_json, cb) => {
                debug!(target: "payments_command_executor", "ParseVerifyPaymentResponse command received");
                self.parse_verify_payment_response(&payment_method, &resp_json, cb).await;
            }
            PaymentsCommand::GetRequestInfo(get_auth_rule_response_json, requester_info, fees_json, cb) => {
                debug!(target: "payments_command_executor", "GetRequestInfo command received");
                cb(self.get_request_info(&get_auth_rule_response_json, requester_info, &fees_json));
	        },
            PaymentsCommand::SignWithAddressReq(wallet_handle, address, message, cb) => {
                debug!(target: "payments_command_executor", "SignWithAddressReq command received");
                self.sign_with_address(wallet_handle, &address, message.as_slice(), cb).await;
            },
            PaymentsCommand::VerifyWithAddressReq(address, message, signature, cb) => {
                debug!(target: "payments_command_executor", "VerifyWithAddressReq command received");
                self.verify_with_address(&address, message.as_slice(), signature.as_slice(), cb).await;
            },
        }
    }

    fn register_method(&self, type_: &str, methods: PaymentsMethodCBs) -> IndyResult<()> {
        trace!("register_method >>> type_: {:?}, methods: {:?}", type_, methods);

        self.payments_service.register_payment_method(type_, methods);
        let res = Ok(());

        trace!("register_method << res: {:?}", res);

        res
    }

    async fn create_address<'a>(&'a self, wallet_handle: WalletHandle, type_: &'a str, config: &'a str, cb: Box<dyn Fn(IndyResult<String>) + Send>) {
        trace!("create_address >>> wallet_handle: {:?}, type_: {:?}, config: {:?}", wallet_handle, type_, config);

        match self.wallet_service.check(wallet_handle).map_err(map_err_err!()) {
            Err(err) => return cb(Err(err)),
            _ => ()
        };

        let result = self.payments_service.create_address(wallet_handle, type_, config).await;
        let total_result: IndyResult<String> = match result {
            Ok(res) => {
                //TODO: think about deleting payment_address on wallet save failure
                self.wallet_service.add_record(wallet_handle, &self.wallet_service.add_prefix("PaymentAddress"), &res, &res, &HashMap::new()).map(|_| res)
                    .map_err(IndyError::from)
            }
            Err(err) => Err(err)
        };
        cb(total_result);

        trace!("create_address <<<");
    }

    fn list_addresses(&self, wallet_handle: WalletHandle, cb: Box<dyn Fn(IndyResult<String>) + Send>) {
        trace!("list_addresses >>> wallet_handle: {:?}", wallet_handle);

        match self.wallet_service.search_records(wallet_handle, &self.wallet_service.add_prefix("PaymentAddress"), "{}", &RecordOptions::id_value()) {
            Ok(mut search) => {
                let mut list_addresses: Vec<String> = Vec::new();

                while let Ok(Some(payment_address)) = search.fetch_next_record() {
                    match payment_address.get_value() {
                        Some(value) => list_addresses.push(value.to_string()),
                        None => cb(Err(err_msg(IndyErrorKind::InvalidState, "Record value not found")))
                    }
                }

                let json_string = serde_json::to_string(&list_addresses)
                    .to_indy(IndyErrorKind::InvalidState, "Cannot deserialize List of Payment Addresses");

                cb(json_string);
            }
            Err(err) => cb(Err(err))
        }
        trace!("list_addresses <<<");
    }

    async fn add_request_fees<'a>(&'a self, wallet_handle: WalletHandle, submitter_did: Option<&'a DidValue>, req: &'a str, inputs: &'a str, outputs: &'a str, extra: Option<&'a str>, cb: BoxedCallbackStringStringSend) {
        trace!("add_request_fees >>> wallet_handle: {:?}, submitter_did: {:?}, req: {:?}, inputs: {:?}, outputs: {:?}, extra: {:?}",
               wallet_handle, submitter_did, req, inputs, outputs, extra);
        if let Some(ref did) = submitter_did {
            match self.crypto_service.validate_did(did).map_err(map_err_err!()) {
                Err(err) => return cb(Err(err)),
                _ => ()
            }
        }

        let method_from_inputs = self.payments_service.parse_method_from_inputs(inputs);

        let method = if outputs == "[]" {
            method_from_inputs
        } else {
            let method_from_outputs = self.payments_service.parse_method_from_outputs(outputs);
            PaymentsCommandExecutor::_merge_parse_result(method_from_inputs, method_from_outputs)
        };

        match method {
            Ok(type_) => {
                cb(self.payments_service.add_request_fees(&type_, wallet_handle, submitter_did, req, inputs, outputs, extra)
                    .await
                    .map(|s| (s, type_)))
            }
            Err(error) => {
                cb(Err(error))
            }
        };
        trace!("add_request_fees <<<");
    }

    async fn parse_response_with_fees<'a>(&'a self, type_: &'a str, response: &'a str, cb: Box<dyn Fn(IndyResult<String>) + Send>) {
        trace!("parse_response_with_fees >>> type_: {:?}, response: {:?}", type_, response);
        cb(self.payments_service.parse_response_with_fees(type_, response).await);
        trace!("parse_response_with_fees <<<");
    }

    async fn build_get_payment_sources_request<'a>(&'a self, wallet_handle: WalletHandle, submitter_did: Option<&'a DidValue>, payment_address: &'a str, next: Option<i64>, cb: BoxedCallbackStringStringSend) {
        trace!("build_get_payment_sources_request >>> wallet_handle: {:?}, submitter_did: {:?}, payment_address: {:?}", wallet_handle, submitter_did, payment_address);
        if let Some(ref did) = submitter_did {
            match self.crypto_service.validate_did(did).map_err(map_err_err!()) {
                Err(err) => return cb(Err(err)),
                _ => ()
            }
        }

        let method = match self.payments_service.parse_method_from_payment_address(payment_address) {
            Ok(method) => method,
            Err(err) => {
                cb(Err(err));
                return;
            }
        };

        cb(self.payments_service.build_get_payment_sources_request(&method, wallet_handle, submitter_did, payment_address, next)
            .await
            .map(|s| (s, method)));
        trace!("build_get_payment_sources_request <<<");
    }

    async fn parse_get_payment_sources_response<'a>(&'a self, type_: &'a str, response: &'a str, cb: Box<dyn Fn(IndyResult<(String, i64)>) + Send>) {
        trace!("parse_get_payment_sources_response >>> response: {:?}", response);
        cb(self.payments_service.parse_get_payment_sources_response(type_, response).await);
        trace!("parse_get_payment_sources_response <<<");
    }

    async fn build_payment_req<'a>(&'a self, wallet_handle: WalletHandle, submitter_did: Option<&'a DidValue>, inputs: &'a str, outputs: &'a str, extra: Option<&'a str>, cb: BoxedCallbackStringStringSend) {
        trace!("build_payment_req >>> wallet_handle: {:?}, submitter_did: {:?}, inputs: {:?}, outputs: {:?}, extra: {:?}", wallet_handle, submitter_did, inputs, outputs, extra);
        if let Some(ref did) = submitter_did {
            match self.crypto_service.validate_did(did).map_err(map_err_err!()) {
                Err(err) => return cb(Err(err)),
                _ => ()
            }
        }

        let method_from_inputs = self.payments_service.parse_method_from_inputs(inputs);
        let method_from_outputs = self.payments_service.parse_method_from_outputs(outputs);
        let method = PaymentsCommandExecutor::_merge_parse_result(method_from_inputs, method_from_outputs);

        match method {
            Ok(type_) => {
                cb(self.payments_service.build_payment_req(&type_, wallet_handle, submitter_did, inputs, outputs, extra)
                       .await
                       .map(|s| (s, type_)));
            }
            Err(error) => {
                cb(Err(error))
            }
        }
        trace!("build_payment_req <<<");
    }

    fn append_txn_author_agreement_acceptance_to_extra(&self,
                                                       extra: Option<&str>,
                                                       text: Option<&str>,
                                                       version: Option<&str>,
                                                       taa_digest: Option<&str>,
                                                       mechanism: &str,
                                                       time: u64) -> IndyResult<String> {
        debug!("append_txn_author_agreement_acceptance_to_extra >>> extra: {:?}, text: {:?}, version: {:?}, taa_digest: {:?}, mechanism: {:?}, time: {:?}",
               extra, text, version, taa_digest, mechanism, time);

        let mut extra: serde_json::Value = serde_json::from_str(extra.unwrap_or("{}"))
            .map_err(|err| IndyError::from_msg(IndyErrorKind::InvalidStructure, format!("Cannot deserialize extra: {:?}", err)))?;

        let acceptance_data = self.ledger_service.prepare_acceptance_data(text, version, taa_digest, mechanism, time)?;

        extra["taaAcceptance"] = serde_json::to_value(acceptance_data)
            .to_indy(IndyErrorKind::InvalidState, "Can't serialize author agreement acceptance data")?;

        let res: String = extra.to_string();

        debug!("append_txn_author_agreement_acceptance_to_extra <<< res: {:?}", res);

        Ok(res)
    }

    async fn parse_payment_response<'a>(&'a self, payment_method: &'a str, response: &'a str, cb: Box<dyn Fn(IndyResult<String>) + Send>) {
        trace!("parse_payment_response >>> response: {:?}", response);
        cb(self.payments_service.parse_payment_response(payment_method, response).await);
        trace!("parse_payment_response <<<");
    }

    async fn build_mint_req<'a>(&'a self, wallet_handle: WalletHandle, submitter_did: Option<&'a DidValue>, outputs: &'a str, extra: Option<&'a str>, cb: BoxedCallbackStringStringSend) {
        trace!("build_mint_req >>> wallet_handle: {:?}, submitter_did: {:?}, outputs: {:?}, extra: {:?}", wallet_handle, submitter_did, outputs, extra);
        if let Some(ref did) = submitter_did {
            match self.crypto_service.validate_did(did).map_err(map_err_err!()) {
                Err(err) => return cb(Err(err)),
                _ => ()
            }
        }

        match self.payments_service.parse_method_from_outputs(outputs) {
            Ok(type_) => {
                cb(self.payments_service.build_mint_req(&type_, wallet_handle, submitter_did, outputs, extra)
                       .await
                       .map(|s| (s, type_)));
            }
            Err(error) => cb(Err(error))
        }
        trace!("build_mint_req <<<");
    }

    async fn build_set_txn_fees_req<'a>(&'a self, wallet_handle: WalletHandle, submitter_did: Option<&'a DidValue>, type_: &'a str, fees: &'a str, cb: Box<dyn Fn(IndyResult<String>) + Send>) {
        trace!("build_set_txn_fees_req >>> wallet_handle: {:?}, submitter_did: {:?}, type_: {:?}, fees: {:?}", wallet_handle, submitter_did, type_, fees);
        if let Some(ref did) = submitter_did {
            match self.crypto_service.validate_did(did).map_err(map_err_err!()) {
                Err(err) => return cb(Err(err)),
                _ => ()
            }
        }

        match serde_json::from_str::<HashMap<String, i64>>(fees) {
            Err(err) => {
                error!("Cannot deserialize Fees: {:?}", err);
                cb(Err(err.to_indy(IndyErrorKind::InvalidStructure, "Cannot deserialize Fees")))
            }
            _ => cb(self.payments_service.build_set_txn_fees_req(type_, wallet_handle, submitter_did, fees).await),
        };
        trace!("build_set_txn_fees_req <<<");
    }

    async fn build_get_txn_fees_req<'a>(&'a self, wallet_handle: WalletHandle, submitter_did: Option<&'a DidValue>, type_: &'a str, cb: Box<dyn Fn(IndyResult<String>) + Send>) {
        trace!("build_get_txn_fees_req >>> wallet_handle: {:?}, submitter_did: {:?}, type_: {:?}", wallet_handle, submitter_did, type_);
        if let Some(ref did) = submitter_did {
            match self.crypto_service.validate_did(did).map_err(map_err_err!()) {
                Err(err) => return cb(Err(err)),
                _ => ()
            }
        }

        cb(self.payments_service.build_get_txn_fees_req(type_, wallet_handle, submitter_did).await);
        trace!("build_get_txn_fees_req <<<");
    }

    async fn parse_get_txn_fees_response<'a>(&'a self, type_: &'a str, response: &'a str, cb: Box<dyn Fn(IndyResult<String>) + Send>) {
        trace!("parse_get_txn_fees_response >>> response: {:?}", response);
        cb(self.payments_service.parse_get_txn_fees_response(type_, response).await);
        trace!("parse_get_txn_fees_response <<<");
    }

    async fn build_verify_payment_request<'a>(&'a self, wallet_handle: WalletHandle, submitter_did: Option<&'a DidValue>, receipt: &'a str, cb: BoxedCallbackStringStringSend) {
        trace!("build_verify_payment_request >>> wallet_handle: {:?}, submitter_did: {:?}, receipt: {:?}", wallet_handle, submitter_did, receipt);
        if let Some(ref did) = submitter_did {
            match self.crypto_service.validate_did(did).map_err(map_err_err!()) {
                Err(err) => return cb(Err(err)),
                _ => ()
            }
        }

        let method = match self.payments_service.parse_method_from_payment_address(receipt) {
            Ok(method) => method,
            Err(err) => {
                cb(Err(err));
                return;
            }
        };
        cb(self.payments_service.build_verify_payment_req(&method, wallet_handle, submitter_did, receipt)
            .await
            .map(|s| (s, method)));
        trace!("build_verify_payment_request <<<");
    }

    async fn parse_verify_payment_response<'a>(&'a self, type_: &'a str, resp_json: &'a str, cb: Box<dyn Fn(IndyResult<String>) + Send>) {
        trace!("parse_verify_payment_response >>> response: {:?}", resp_json);
        cb(self.payments_service.parse_verify_payment_response(type_, resp_json).await);
        trace!("parse_verify_payment_response <<<");
    }

    async fn sign_with_address<'a>(&'a self, wallet_handle: WalletHandle, address: &'a str, message: &'a [u8], cb: Box<dyn Fn(IndyResult<Vec<u8>>) + Send>) {
        trace!("sign_with_address >>> address: {:?}, message: {:?}", address, hex::encode(message));
        let method = match self.payments_service.parse_method_from_payment_address(address) {
            Ok(method) => method,
            Err(err) => {
                cb(Err(err));
                return;
            }
        };

        cb(self.payments_service.sign_with_address(&method, wallet_handle, address, message).await);
    }

    async fn verify_with_address<'a>(&'a self, address: &'a str, message: &'a [u8], signature: &'a [u8], cb: Box<dyn Fn(IndyResult<bool>) + Send>) {
        trace!("sign_with_address >>> address: {:?}, message: {:?}, signature: {:?}", address, hex::encode(message), hex::encode(signature));

        let method = match self.payments_service.parse_method_from_payment_address(address) {
            Ok(method) => method,
            Err(err) => {
                cb(Err(err));
                return;
            }
        };

        cb(self.payments_service.verify_with_address(&method, address, message, signature).await);
    }


    // HELPERS

    fn _merge_parse_result(method_from_inputs: IndyResult<String>, method_from_outputs: IndyResult<String>) -> IndyResult<String> {
        match (method_from_inputs, method_from_outputs) {
            (Err(err), _) | (_, Err(err)) => Err(err),
            (Ok(ref mth1), Ok(ref mth2)) if mth1 != mth2 => {
                error!("Different payment method in inputs and outputs");
                Err(err_msg(IndyErrorKind::IncompatiblePaymentMethods, "Different payment method in inputs and outputs"))
            }
            (Ok(mth1), Ok(_)) => Ok(mth1)
        }
    }

    pub fn get_request_info(&self, get_auth_rule_response_json: &str, requester_info: RequesterInfo, fees: &Fees) -> IndyResult<String> {
        trace!("get_request_info >>> get_auth_rule_response_json: {:?}, requester_info: {:?}, fees: {:?}", get_auth_rule_response_json, requester_info, fees);

        let auth_rule = self._parse_get_auth_rule_response(get_auth_rule_response_json)?;

        let req_info = self.payments_service.get_request_info_with_min_price(&auth_rule.constraint, &requester_info, &fees)?;

        let res = serde_json::to_string(&req_info)
            .to_indy(IndyErrorKind::InvalidState, "Cannot serialize RequestInfo")?;

        trace!("get_request_info <<< {:?}", res);

        Ok(res)
    }

    fn _parse_get_auth_rule_response(&self, get_auth_rule_response_json: &str) -> IndyResult<AuthRule> {
        trace!("_parse_get_auth_rule_response >>> get_auth_rule_response_json: {:?}", get_auth_rule_response_json);

        let mut auth_rules: Vec<AuthRule> = self.ledger_service.parse_get_auth_rule_response(get_auth_rule_response_json)?;

        if auth_rules.len() != 1 {
            return Err(IndyError::from_msg(IndyErrorKind::InvalidTransaction, "GetAuthRule response must contain one auth rule"));
        }

        let res = auth_rules.pop().unwrap();

        trace!("_parse_get_auth_rule_response <<< {:?}", res);

        Ok(res)
    }
}
