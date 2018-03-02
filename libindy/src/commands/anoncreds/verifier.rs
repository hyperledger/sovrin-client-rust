extern crate serde_json;
extern crate indy_crypto;

use errors::common::CommonError;
use errors::indy::IndyError;

use services::anoncreds::AnoncredsService;
use services::anoncreds::types::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use self::indy_crypto::cl::RevocationRegistry;
use self::indy_crypto::utils::json::JsonDecodable;

pub enum VerifierCommand {
    VerifyProof(
        String, // proof request json
        String, // proof json
        String, // credential schemas json
        String, // credential defs jsons
        String, // rev reg defs json
        String, // rev reg json
        Box<Fn(Result<bool, IndyError>) + Send>)
}

pub struct VerifierCommandExecutor {
    anoncreds_service: Rc<AnoncredsService>,
}

impl VerifierCommandExecutor {
    pub fn new(anoncreds_service: Rc<AnoncredsService>) -> VerifierCommandExecutor {
        VerifierCommandExecutor {
            anoncreds_service,
        }
    }

    pub fn execute(&self, command: VerifierCommand) {
        match command {
            VerifierCommand::VerifyProof(proof_request_json, proof_json, credential_schemas_json, credential_defs_json, rev_reg_defs_json, rev_regs_json, cb) => {
                trace!(target: "verifier_command_executor", "VerifyProof command received");
                cb(self.verify_proof(&proof_request_json, &proof_json, &credential_schemas_json, &credential_defs_json, &rev_reg_defs_json, &rev_regs_json));
            }
        };
    }

    fn verify_proof(&self,
                    proof_request_json: &str,
                    proof_json: &str,
                    credential_schemas_json: &str,
                    credential_defs_json: &str,
                    rev_reg_defs_json: &str,
                    rev_regs_json: &str) -> Result<bool, IndyError> {
        trace!("verify_proof >>> proof_request_json: {:?}, proof_json: {:?}, credential_schemas_json: {:?}, credential_defs_json: {:?},  \
               rev_reg_defs_json: {:?}, rev_regs_json: {:?}",
               proof_request_json, proof_json, credential_schemas_json, credential_defs_json, rev_reg_defs_json, rev_regs_json);

        let proof_req: ProofRequest = ProofRequest::from_json(proof_request_json)
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize ProofRequest: {:?}", err)))?;

        let credential_schemas: HashMap<String, Schema> = serde_json::from_str(credential_schemas_json)
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize list of Schema: {:?}", err)))?;

        let credential_defs: HashMap<String, CredentialDefinition> = serde_json::from_str(credential_defs_json)
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize list of CredentialDefinition: {:?}", err)))?;

        let rev_reg_defs: HashMap<String, RevocationRegistryDefinitionValue> = serde_json::from_str(rev_reg_defs_json)
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize list of RevocationRegistryDef: {:?}", err)))?;

        let rev_regs: HashMap<String, RevocationRegistry> = serde_json::from_str(rev_regs_json)
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize list of RevocationRegistry: {:?}", err)))?;

        let proof_claims: FullProof = FullProof::from_json(&proof_json)
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize Proof: {:?}", err)))?;

        if credential_schemas.keys().collect::<HashSet<&String>>() != credential_defs.keys().collect::<HashSet<&String>>() {
            return Err(IndyError::CommonError(CommonError::InvalidStructure(
                format!("CredentialDefinitions {:?} do not correspond to Schema {:?}", credential_schemas.keys(), credential_defs.keys()))));
        }

        let requested_attrs: HashSet<String> =
            proof_req.requested_attrs
                .keys()
                .map(|referent| referent.clone())
                .into_iter()
                .collect::<HashSet<String>>();

        let received_revealed_attrs: HashSet<String> =
            proof_claims.requested_proof.revealed_attrs
                .keys()
                .map(|referent| referent.clone())
                .into_iter()
                .collect::<HashSet<String>>();

        let received_unrevealed_attrs: HashSet<String> =
            proof_claims.requested_proof.unrevealed_attrs
                .keys()
                .map(|referent| referent.clone())
                .into_iter()
                .collect::<HashSet<String>>();

        let received_self_attested_attrs: HashSet<String> =
            proof_claims.requested_proof.self_attested_attrs
                .keys()
                .map(|referent| referent.clone())
                .into_iter()
                .collect::<HashSet<String>>();

        let received_attrs = received_revealed_attrs
            .union(&received_unrevealed_attrs)
            .map(|attr| attr.clone())
            .collect::<HashSet<String>>()
            .union(&received_self_attested_attrs)
            .map(|attr| attr.clone())
            .collect::<HashSet<String>>();

        if requested_attrs != received_attrs {
            return Err(IndyError::CommonError(CommonError::InvalidStructure(
                format!("Requested attributes {:?} do not correspond to received {:?}", requested_attrs, received_attrs))));
        }

        let requested_predicates: HashSet<String> =
            proof_req.requested_predicates
                .keys()
                .map(|referent| referent.clone())
                .into_iter()
                .collect::<HashSet<String>>();

        let received_predicates: HashSet<String> =
            proof_claims.requested_proof.predicates
                .keys()
                .map(|referent| referent.clone())
                .into_iter()
                .collect::<HashSet<String>>();

        if requested_predicates != received_predicates {
            return Err(IndyError::CommonError(CommonError::InvalidStructure(
                format!("Requested predicates {:?} do not correspond to received {:?}", requested_predicates, received_predicates))));
        }

        let result = self.anoncreds_service.verifier.verify(&proof_claims,
                                                            &proof_req,
                                                            &credential_schemas,
                                                            &credential_defs,
                                                            &rev_reg_defs,
                                                            &rev_regs)?;

        trace!("verify_proof <<< result: {:?}", result);

        Ok(result)
    }
}