#![allow(dead_code)] // TODO: Remove later

use sgx_types::{sgx_status_t, sgx_enclave_id_t};
use enigma_types::EnclaveReturn;
use enigma_types::traits::SliceCPtr;
use failure::Error;
use crate::common_u::errors::EnclaveFailError;
use std::mem;

pub type ContractAddress = [u8; 32];
pub type StateKey = [u8; 32];
pub type PubKey = [u8; 64];

extern "C" {
    fn ecall_ptt_req(eid: sgx_enclave_id_t, retval: *mut EnclaveReturn, addresses: *const ContractAddress, len: usize,
                     signature: &mut [u8; 65], serialized_ptr: *mut u64) -> sgx_status_t;
    fn ecall_ptt_res(eid: sgx_enclave_id_t, retval: *mut EnclaveReturn, msg_ptr: *const u8, msg_len: usize) -> sgx_status_t;
    fn ecall_build_state(eid: sgx_enclave_id_t, retval: *mut EnclaveReturn, failed_ptr: *mut u64) -> sgx_status_t;
    fn ecall_get_user_key(eid: sgx_enclave_id_t, retval: *mut EnclaveReturn,
                          signature: &mut [u8; 65], user_pubkey: &PubKey, serialized_ptr: *mut u64) -> sgx_status_t;

}

pub fn ptt_build_state(eid: sgx_enclave_id_t) -> Result<Vec<ContractAddress>, Error> {
    let mut ret = EnclaveReturn::Success;
    let mut failed_ptr = 0u64;
    let status = unsafe { ecall_build_state(eid, &mut ret as *mut EnclaveReturn, &mut failed_ptr as *mut u64) };
    if ret != EnclaveReturn::Success  || status != sgx_status_t::SGX_SUCCESS {
        return Err(EnclaveFailError{err: ret, status}.into());
    }
    let box_ptr = failed_ptr as *mut Box<[u8]>;
    let part = unsafe { Box::from_raw(box_ptr) };
    let part: Vec<ContractAddress> = part.chunks(32).map(|s| {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(s);
        arr
    }).collect();
    Ok(part)
}


pub fn ptt_res(eid: sgx_enclave_id_t, msg: &[u8]) -> Result<(), Error> {
    let mut ret = EnclaveReturn::Success;
    let status = unsafe { ecall_ptt_res(eid, &mut ret as *mut EnclaveReturn, msg.as_c_ptr(), msg.len()) };
    if ret != EnclaveReturn::Success  || status != sgx_status_t::SGX_SUCCESS {
        return Err(EnclaveFailError{err: ret, status}.into());
    }
    Ok(())
}


pub fn ptt_req(eid: sgx_enclave_id_t, addresses: &[ContractAddress]) -> Result<(Box<[u8]>, [u8; 65]), Error> {
    let mut sig = [0u8; 65];
    let mut ret = EnclaveReturn::default();
    let mut serialized_ptr = 0u64;

    let status = unsafe { ecall_ptt_req(eid,
                           &mut ret as *mut EnclaveReturn,
                           addresses.as_c_ptr() as *const ContractAddress,
                           addresses.len() * mem::size_of::<ContractAddress>(),
                           &mut sig,
                           &mut serialized_ptr as *mut u64
    )};
    if ret != EnclaveReturn::Success  || status != sgx_status_t::SGX_SUCCESS {
        return Err(EnclaveFailError{err: ret, status}.into());
    }
    let box_ptr = serialized_ptr as *mut Box<[u8]>;
    let part = unsafe { Box::from_raw(box_ptr) };
    Ok( (*part, sig) )
}


pub fn get_user_key(eid: sgx_enclave_id_t, user_pubkey: &PubKey) -> Result<(Box<[u8]>, [u8; 65]), Error> {
    let mut sig = [0u8; 65];
    let mut ret = EnclaveReturn::Success;
    let mut serialized_ptr = 0u64;

    let status = unsafe { ecall_get_user_key(eid,
                                        &mut ret as *mut EnclaveReturn,
                                        &mut sig,
                                        &user_pubkey,
                                        &mut serialized_ptr as *mut u64
    )};
    if ret != EnclaveReturn::Success  || status != sgx_status_t::SGX_SUCCESS {
        return Err(EnclaveFailError{err: ret, status}.into());
    }
    let box_ptr = serialized_ptr as *mut Box<[u8]>;
    let part = unsafe { Box::from_raw(box_ptr) };
    Ok((*part, sig))
}

#[cfg(test)]
pub mod tests {
    extern crate secp256k1;
    extern crate ring;

    use crate::esgx::general::init_enclave_wrapper;
    use super::{ContractAddress, StateKey, ptt_req, ptt_res, ptt_build_state};
    use crate::db::{DeltaKey, DATABASE, CRUDInterface};
    use crate::db::Stype::{Delta, State};
    use super::PubKey;
    use enigma_tools_u::common_u::{Sha256, Keccak256};
    use rmp_serde::{Deserializer, Serializer};
    use serde::{Deserialize, Serialize};
    use self::secp256k1::{PublicKey, SecretKey, SharedSecret, Message, Signature, RecoveryId};
    use serde_json::{self, Value};
    use self::ring::{aead, rand::*};
    use sgx_types::sgx_enclave_id_t;
    use std::collections::HashSet;

    const PUBKEY_DUMMY: [u8; 64] = [ 27, 132, 197, 86, 123, 18, 100, 64, 153, 93, 62, 213, 170, 186, 5, 101, 215, 30, 24, 52, 96, 72, 25, 255, 156, 23, 245, 233, 213, 221, 7, 143, 112, 190, 175, 143, 88, 139, 84, 21, 7, 254, 214, 166, 66, 197, 171, 66, 223, 223, 129, 32, 167, 246, 57, 222, 81, 34, 212, 122, 105, 168, 232, 209];

    pub fn exchange_keys(id: sgx_enclave_id_t) -> (PubKey, Box<[u8]>, [u8; 65]) {
        let mut _priv = [0u8; 32];
        SystemRandom::new().fill(&mut _priv).unwrap();
        let privkey = SecretKey::parse(&_priv).unwrap();
        let _pubkey = PublicKey::from_secret_key(&privkey);
        let mut pubkey = [0u8; 64];
        pubkey.clone_from_slice(&_pubkey.serialize()[1..]);
        let (data, sig) = super::get_user_key(id, &pubkey).unwrap();
        (pubkey, data, sig)
    }

    #[test]
    fn test_get_user_key() {
        let enclave = init_enclave_wrapper().unwrap();
        let (_, data, _sig) = exchange_keys(enclave.geteid());

        let mut des = Deserializer::new(&data[..]);
        let res: Value = Deserialize::deserialize(&mut des).unwrap();
        let prefix = serde_json::from_value::<[u8; 19]>(res["prefix"].clone()).unwrap();
        assert_eq!(b"Enigma User Message", &prefix);

        let mut sig = [0u8; 64];
        sig.copy_from_slice(&_sig[..64]);
        let sig = Signature::parse(&sig);

        let msg = Message::parse(&data.keccak256());
        let recovery_id = RecoveryId::parse(_sig[64]-27).unwrap();
        let _pubkey = secp256k1::recover(&msg, &sig, &recovery_id).unwrap();
        // TODO: Consider verifying this against ecall_get_signing_address
    }

    #[test]
    fn test_ptt_req() {
        let enclave = init_enclave_wrapper().unwrap();
        let addresses: [ContractAddress; 3] = [[1u8 ;32], [2u8; 32], [3u8; 32]];
        let (msg, sig) = ptt_req(enclave.geteid(), &addresses).unwrap();
        assert_ne!(msg.len(), 0);
        assert_ne!(sig.to_vec(), vec![0u8; 64]);
    }

    pub fn instantiate_encryption_key(addresses: &[ContractAddress], eid: sgx_enclave_id_t) {
        let req = ptt_req(eid, &addresses).unwrap();

        let mut des = Deserializer::new(&req.0[..]);
        let req_val: Value = Deserialize::deserialize(&mut des).unwrap();

        let enc_response = make_encrpted_resposnse(req_val);

        let mut serialized_enc_response = Vec::new();
        enc_response.serialize(&mut Serializer::new(&mut serialized_enc_response)).unwrap();

        ptt_res(eid, &serialized_enc_response).unwrap();

    }

    fn make_encrpted_resposnse(req: Value) -> Value {
        // Making the response
        let req_data: Vec<ContractAddress> = serde_json::from_value(req["data"]["Request"].clone()).unwrap();
        let _response_data: Vec<(ContractAddress, StateKey)>  = req_data.into_iter().map(|add| (add, add.sha256())).collect();

        let mut response_data = Vec::new();
        _response_data.serialize(&mut Serializer::new(&mut response_data)).unwrap();


        // Getting the node DH Public Key
        let _pubkey: Vec<u8> = serde_json::from_value(req["pubkey"].clone()).unwrap();
        let mut pubkey = [0u8; 65];
        pubkey[0] = 4;
        pubkey[1..].copy_from_slice(&_pubkey);
        let node_pubkey = PublicKey::parse(&pubkey).unwrap();

        // Generating a second pair of priv-pub keys for the DH
        let km_priv_key = SecretKey::parse(&b"Enigma".sha256()).unwrap();
        let km_pubkey = PublicKey::from_secret_key(&km_priv_key);

        // Generating the ECDH key for AES
        let shared = SharedSecret::new(&node_pubkey, &km_priv_key).unwrap();
        let seal_key = aead::SealingKey::new(&aead::AES_256_GCM, shared.as_ref()).unwrap();

        // Encrypting the response
        let iv = [1u8; 12];
        response_data.extend(vec![0u8; aead::AES_256_GCM.tag_len()]);
        let s = aead::seal_in_place(&seal_key, &iv, &[], &mut response_data, aead::AES_256_GCM.tag_len()).unwrap();
        assert_eq!(s, response_data.len());
        response_data.extend(&iv);

        // Building the Encrypted Response.
        let mut enc_template: Value = serde_json::from_str(
            "{\"data\":{\
                    \"EncryptedResponse\":[239,255,23,228,191,26,143,198,128,188,100,241,178,217,234,168,108,235,78,65,238,186,149,171,226,107,165,133,44,177,27,14,128,38,137,97,202,160,120,230,88,226,218,127,41,16,29,135,167,0,186,110,21,164,73,226,244,202,243,227,78,75,216,216,138,135,158,26,136,143,45,118,11,248,0,66,204,94,63,193,31,148,110,58,35,104,219,233,159,244,176,244,33,8,214,223,107,103,44,243,28,237,155,104,3,243,217,122,233,16,192,163,112,164,66,250,116,194,45,111,174,65,142,179,228,132,195,118,123,34,219,135,245,83,113,8,141,6,241,156,136,70,134,206,238,227,26,106,248,215,20,130,181,231,216,193,238,87,241,150,14,45,180,22,191,100,207,148,82,89,5,158,241,173,193,140,214,109,139,18,91,200,251,121,16,119,21,243,177,104,46,254,48,41,115,56,8,37,27,155,95,51,125,244,75,154,90,47,181,110,126,174,96,90,25,34,92,89,250,240,5,200,147,228,148,158,193,54,12,249,243,47,172,27,131,158,32,167,116,200,110,29,151,13,78,23,41,199,188,127,142,109,3,130,202,179,168,111,128,246,242,23,7,247,87,151,110,102,30,226,94,135,249,244,48,250,32,177,155,28,217,175,25,89,231,167,1,54,204,124,20,196,168,239,148,200,45,213,185,37,144,138,244,194,211,141,5,171,93,146,138,154,5,4,243,9,123,237,186,233,215,42,121,152,75,208,13,156,53,86,254,123,182,21,210,230,235,237,12]\
                },\
                \"id\":[99,31,224,64,105,252,120,51,200,241,224,56],\
                \"prefix\":[69,110,105,103,109,97,32,77,101,115,115,97,103,101],\
                \"pubkey\":[127,228,135,71,145,246,191,25,182,250,194,154,40,157,166,47,6,214,203,209,7,71,48,253,171,195,26,131,255,59,181,47,202,186,164,88,190,47,24,102,237,57,130,227,253,190,12,121,200,130,221,255,42,121,136,131,170,143,132,174,21,219,245,153]\
            }"
        ).unwrap();
        enc_template["data"]["EncryptedResponse"] = json!(response_data);
        enc_template["id"] = req["id"].clone();
        let km_pubkey_slice = km_pubkey.serialize()[1..65].to_vec();
        enc_template["pubkey"] = json!(km_pubkey_slice);

        enc_template
    }

    #[test]
    fn test_the_whole_round() {
        //Making a request
        let address = fill_the_db();
        let enclave = init_enclave_wrapper().unwrap();
        let req = ptt_req(enclave.geteid(), &address).unwrap();

        // serializing the result from the request
        let mut des = Deserializer::new(&req.0[..]);
        let req_val: Value = Deserialize::deserialize(&mut des).unwrap();

        // Generating the response
        let enc_response = make_encrpted_resposnse(req_val);

        let mut serialized_enc_response = Vec::new();
        enc_response.serialize(&mut Serializer::new(&mut serialized_enc_response)).unwrap();

        ptt_res(enclave.geteid(), &serialized_enc_response).unwrap();

        // Testing equality while ignoring order.
        let address_result = ptt_build_state(enclave.geteid()).unwrap();
        assert_eq!(address_result.len(), address.len());
        let address_set: HashSet<&ContractAddress> = address.iter().collect();
        assert!(address_result.iter().all(|x| address_set.contains(x)));
    }

    fn fill_the_db() -> Vec<[u8; 32]> {
        let address = vec![b"first".sha256(), b"second".sha256(), b"third".sha256()];
        let stuff = vec![
            (DeltaKey { hash: address[0], key_type: State }, vec![192, 142, 76, 156, 249, 151, 23, 224, 217, 73, 57, 184, 8, 162, 146, 11, 127, 64, 18, 148, 192, 53, 158, 32, 109, 26, 198, 242, 94, 64, 65, 245, 93, 0, 48, 165, 1, 241, 66, 253, 245, 215, 44, 168, 221, 242, 157, 187, 153, 238, 217, 64, 201, 237, 69, 178, 192, 27, 26, 44, 182, 112, 227, 165, 183, 34, 186, 218, 208, 105, 51, 187, 188, 24, 172, 84, 114, 194, 200, 86, 250, 198, 45, 250, 216, 221, 40, 62, 207, 88, 66, 137, 246, 217, 41, 31, 59, 88, 133, 114, 199, 214, 133, 65, 208, 61, 77, 212, 174, 127, 206, 129, 230, 55, 203, 101, 228, 71, 193, 68, 153, 237, 91, 87, 14, 211, 106, 105, 6, 14, 38, 114, 62, 111, 194, 40, 173, 147, 86, 251, 122, 37, 85, 149, 39, 49, 230, 226, 139, 134, 10, 87, 135, 45, 6, 207, 35, 97, 93, 39, 146, 133, 100, 195, 57, 57, 130, 218, 84, 198, 37, 52, 10, 111, 136, 207, 111, 173, 194, 69, 139, 174, 201, 179, 247, 215, 54, 99, 16, 228, 138, 214, 203, 95, 216, 27, 96, 148, 36, 75, 47, 139, 73, 84, 99, 91, 246, 250, 212, 16, 100, 37, 153, 3, 253, 185, 34, 146, 114, 137, 93, 104, 84, 241, 184, 53, 216, 78, 24, 90, 113, 145, 255, 247, 204, 154, 141, 188, 112, 161, 190, 37, 201, 23, 207, 189, 214, 255, 186, 97, 118, 58, 82, 54, 190, 60, 252, 154, 110, 183, 217, 191, 111, 67, 77, 29, 102, 77, 241, 254, 98, 81, 58, 203, 143, 206, 49, 153, 182, 75, 14, 225, 122, 105, 75, 115, 194, 166, 55, 92, 131, 215, 176, 74, 49, 142, 114, 57, 193, 210, 235, 24, 20, 113, 109, 155, 124, 250, 161, 162, 51, 199, 207, 91, 54, 110, 215, 8, 237, 132, 68, 77, 184, 214, 162, 185, 203, 139, 105, 198, 45, 244, 47, 110, 197, 207, 70, 116, 171, 141, 0, 119, 9, 209, 104, 89, 43, 225, 214, 13, 62, 54, 171, 25, 27, 157, 49, 118, 248, 254, 56, 232, 144, 15, 136, 99, 2, 172, 135, 40, 217, 100]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![166, 14, 29, 59, 169, 196, 250, 191, 136, 248, 13, 215, 169, 211, 48, 241, 57, 238, 140, 170, 65, 113, 159, 248, 102, 137, 235, 191, 178, 191, 105, 248, 5, 122, 134, 23, 81, 95, 78, 86, 86, 3, 46, 56, 165, 118, 156, 160, 1, 233, 212, 236, 116, 252, 190, 224, 131, 184, 127, 162, 204, 159, 169, 132, 185, 212, 139, 99, 1, 104, 107, 180, 103, 13]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![208, 196, 61, 25, 95, 207, 126, 54, 133, 191, 208, 236, 168, 107, 82, 48, 217, 38, 96, 120, 59, 123, 172, 154, 20, 187, 65, 114, 65, 19, 166, 177, 110, 17, 187, 22, 129, 102, 131, 72, 221, 2, 142, 188, 251, 110, 2, 140, 243, 249, 231, 140, 161, 203, 181, 2, 124, 20, 87, 180, 206, 107, 212, 156, 231, 223, 75, 84, 5, 77, 140, 85, 117, 3]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![250, 191, 25, 86, 239, 92, 96, 221, 211, 110, 75, 24, 46, 253, 42, 223, 136, 216, 203, 69, 136, 236, 46, 245, 91, 197, 34, 93, 34, 197, 171, 33, 199, 104, 180, 112, 63, 13, 175, 34, 163, 84, 100, 124, 34, 27, 113, 119, 44, 20, 218, 86, 171, 223, 215, 217, 185, 232, 39, 39, 140, 123, 154, 56, 172, 220, 122, 18, 50, 25, 250, 196, 146, 127]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![253, 146, 70, 111, 186, 135, 211, 194, 218, 17, 162, 17, 119, 43, 196, 54, 99, 49, 142, 221, 169, 97, 117, 133, 102, 155, 221, 142, 179, 81, 203, 182, 167, 68, 238, 57, 218, 63, 218, 152, 223, 112, 98, 29, 76, 79, 196, 8, 130, 144, 8, 210, 15, 174, 80, 106, 93, 22, 254, 184, 0, 3, 120, 72, 10, 95, 227, 215, 220, 90, 179, 97, 27, 62]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![137, 238, 254, 187, 183, 191, 243, 40, 99, 18, 208, 93, 62, 35, 239, 170, 108, 236, 141, 125, 139, 79, 254, 2, 12, 150, 67, 145, 32, 246, 184, 54, 248, 97, 219, 22, 250, 212, 255, 51, 182, 242, 196, 94, 127, 241, 249, 131, 192, 164, 107, 179, 252, 235, 37, 135, 26, 11, 157, 114, 245, 164, 60, 249, 27, 225, 46, 71, 218, 14, 161, 144, 132, 229]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![159, 218, 182, 194, 93, 145, 168, 215, 80, 54, 139, 50, 173, 23, 244, 221, 136, 14, 28, 182, 119, 244, 69, 108, 230, 18, 235, 81, 224, 33, 170, 38, 137, 79, 209, 42, 52, 209, 75, 39, 49, 111, 13, 133, 65, 132, 98, 24, 156, 230, 214, 196, 187, 54, 138, 118, 156, 107, 79, 51, 131, 47, 229, 114, 168, 235, 177, 112, 211, 209, 242, 102, 215, 244]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![217, 201, 56, 130, 251, 1, 85, 252, 188, 12, 20, 157, 148, 88, 228, 210, 114, 122, 162, 30, 195, 140, 209, 146, 113, 138, 92, 206, 180, 116, 201, 111, 71, 90, 28, 123, 94, 62, 92, 56, 216, 47, 191, 239, 149, 36, 74, 87, 181, 218, 186, 203, 164, 122, 113, 125, 104, 122, 115, 194, 172, 162, 25, 250, 75, 84, 107, 116, 134, 167, 55, 131, 29, 96]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![192, 32, 124, 211, 124, 184, 172, 147, 252, 164, 107, 54, 177, 161, 77, 49, 144, 61, 205, 165, 34, 191, 89, 9, 129, 213, 113, 134, 145, 92, 39, 42, 33, 133, 181, 241, 220, 13, 139, 18, 9, 255, 196, 83, 47, 16, 231, 118, 17, 187, 91, 26, 238, 19, 117, 205, 243, 190, 165, 0, 50, 226, 199, 189, 169, 126, 82, 23, 55, 192, 129, 77, 167, 175]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![89, 20, 133, 79, 116, 26, 69, 76, 119, 56, 221, 219, 83, 210, 54, 70, 196, 44, 19, 28, 82, 186, 119, 222, 176, 13, 124, 169, 18, 147, 246, 161, 135, 26, 219, 154, 82, 129, 121, 206, 150, 44, 54, 32, 116, 242, 78, 255, 144, 255, 32, 119, 152, 169, 40, 93, 33, 161, 229, 86, 59, 217, 227, 11, 192, 143, 100, 214, 38, 229, 51, 34, 222, 232]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![231, 208, 107, 199, 66, 93, 250, 58, 7, 241, 245, 92, 18, 16, 203, 132, 18, 91, 20, 23, 221, 173, 9, 139, 201, 10, 195, 255, 206, 233, 69, 36, 200, 127, 148, 142, 123, 36, 221, 133, 38, 8, 229, 120, 123, 97, 62, 125, 31, 134, 208, 138, 51, 30, 66, 202, 171, 183, 13, 173, 184, 187, 33, 182, 220, 139, 239, 139, 198, 26, 146, 198, 251, 29]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![208, 32, 61, 59, 86, 68, 97, 181, 90, 30, 90, 185, 209, 222, 112, 205, 129, 198, 54, 204, 100, 9, 17, 130, 26, 34, 30, 232, 189, 178, 118, 227, 211, 253, 34, 93, 199, 181, 25, 198, 135, 38, 157, 227, 103, 90, 159, 44, 182, 96, 135, 220, 20, 209, 128, 189, 157, 66, 70, 104, 133, 113, 2, 203, 224, 123, 23, 163, 178, 127, 238, 69, 221, 142, 8]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![165, 181, 234, 244, 77, 251, 158, 10, 143, 223, 243, 9, 185, 232, 200, 90, 84, 52, 180, 227, 189, 198, 196, 107, 252, 164, 179, 180, 45, 79, 13, 125, 15, 36, 190, 149, 74, 107, 31, 166, 222, 160, 71, 102, 172, 73, 54, 188, 86, 225, 41, 41, 59, 98, 199, 78, 185, 245, 240, 172, 20, 240, 152, 31, 83, 180, 168, 94, 42, 101, 213, 134, 235, 161, 194]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![67, 123, 102, 91, 202, 118, 17, 81, 21, 216, 91, 18, 18, 59, 86, 216, 18, 180, 41, 94, 181, 48, 234, 123, 123, 80, 20, 59, 193, 174, 182, 33, 71, 97, 68, 206, 119, 30, 83, 64, 255, 84, 29, 103, 119, 210, 98, 146, 168, 55, 115, 254, 34, 243, 192, 123, 16, 33, 40, 169, 59, 78, 236, 82, 217, 211, 164, 116, 223, 15, 173, 222, 251, 213, 231]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![200, 65, 44, 62, 186, 108, 100, 194, 106, 179, 55, 82, 236, 133, 245, 213, 154, 31, 134, 249, 126, 48, 178, 68, 161, 121, 165, 165, 17, 117, 156, 205, 128, 123, 18, 10, 221, 204, 41, 168, 82, 242, 43, 215, 93, 182, 244, 93, 13, 244, 187, 62, 131, 7, 241, 184, 5, 51, 73, 81, 113, 174, 229, 245, 77, 30, 109, 185, 72, 162, 193, 194, 175, 224, 52]),
            (DeltaKey { hash: address[0], key_type: Delta(1) }, vec![238, 62, 56, 19, 170, 171, 43, 105, 136, 88, 88, 100, 5, 59, 169, 160, 200, 46, 85, 239, 92, 201, 154, 202, 253, 248, 69, 247, 178, 133, 199, 28, 144, 179, 69, 78, 237, 143, 171, 145, 245, 76, 161, 120, 174, 232, 25, 227, 59, 22, 61, 150, 99, 210, 243, 23, 222, 5, 61, 213, 42, 122, 23, 3, 125, 232, 204, 68, 133, 104, 38, 212, 163, 163, 28]),
            (DeltaKey { hash: address[1], key_type: State }, vec![106, 30, 98, 51, 242, 248, 11, 154, 62, 5, 165, 16, 241, 227, 2, 251, 116, 81, 1, 239, 171, 72, 78, 13, 174, 147, 125, 139, 141, 120, 114, 171, 132, 95, 147, 116, 215, 252, 79, 43, 138, 143, 120, 108, 87, 24, 128, 84, 39, 4, 70, 144, 172, 108, 90, 112, 19, 231, 58, 228, 76, 91, 146, 170, 173, 78, 139, 149, 3, 71, 129, 126, 200, 236, 46, 157, 47, 62, 183, 88, 145, 202, 171, 219, 123, 201, 57, 37, 243, 102, 181, 219, 151, 249, 110, 114, 35, 162, 132, 102, 131, 99, 53, 220, 140, 13, 24, 245, 36, 79, 60, 61, 60, 248, 200, 42, 179]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![52, 24, 85, 182, 193, 171, 65, 26, 219, 244, 152, 29, 124, 135, 79, 54, 117, 71, 82, 51, 182, 233, 219, 11, 200, 62, 182, 80, 107, 237, 179, 252, 49, 165, 151, 33, 215, 104, 198, 177, 206, 39, 114, 106, 76, 83, 19, 160, 112, 134, 84, 168, 215, 188, 145, 67, 54, 212, 137, 237, 153, 48, 112, 120, 21, 52, 41, 124, 136, 189, 195, 239, 99, 138]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![30, 62, 76, 170, 53, 45, 34, 229, 161, 145, 211, 197, 38, 186, 133, 186, 23, 210, 48, 35, 148, 119, 143, 200, 92, 245, 87, 179, 14, 82, 143, 209, 3, 103, 164, 226, 10, 238, 223, 81, 74, 168, 98, 35, 130, 65, 200, 191, 170, 39, 85, 252, 243, 254, 188, 43, 109, 83, 179, 254, 125, 31, 215, 64, 198, 112, 141, 135, 204, 247, 128, 87, 177, 124]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![164, 195, 205, 100, 91, 210, 95, 150, 197, 98, 181, 205, 41, 99, 86, 242, 29, 144, 190, 168, 73, 56, 23, 103, 34, 55, 134, 84, 239, 99, 214, 154, 125, 209, 8, 115, 196, 63, 55, 179, 28, 157, 212, 76, 221, 162, 205, 130, 104, 193, 27, 235, 223, 226, 66, 58, 95, 108, 60, 85, 43, 14, 154, 26, 47, 155, 174, 212, 194, 8, 48, 132, 148, 71]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![238, 133, 240, 160, 227, 150, 43, 132, 128, 191, 254, 95, 198, 179, 152, 40, 33, 172, 145, 72, 142, 226, 3, 20, 131, 16, 62, 69, 89, 171, 241, 99, 184, 2, 141, 216, 159, 169, 147, 189, 117, 154, 89, 47, 139, 218, 236, 121, 166, 148, 85, 102, 122, 209, 236, 66, 50, 99, 55, 236, 219, 2, 25, 48, 165, 62, 211, 109, 245, 47, 229, 112, 102, 17]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![246, 88, 109, 15, 113, 9, 135, 6, 138, 53, 68, 214, 110, 120, 136, 28, 113, 101, 200, 118, 194, 210, 231, 241, 233, 120, 65, 175, 7, 84, 52, 198, 98, 18, 141, 149, 15, 192, 192, 212, 159, 158, 68, 125, 66, 135, 196, 89, 145, 8, 70, 144, 78, 110, 232, 45, 239, 39, 149, 231, 134, 109, 224, 31, 100, 34, 13, 237, 72, 51, 154, 31, 91, 97]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![192, 37, 189, 111, 106, 227, 253, 186, 29, 134, 37, 158, 107, 93, 168, 63, 87, 60, 46, 169, 63, 8, 11, 94, 149, 15, 159, 135, 247, 197, 130, 123, 87, 201, 217, 223, 205, 143, 45, 178, 166, 156, 16, 64, 37, 93, 243, 100, 240, 46, 85, 140, 216, 144, 68, 106, 213, 189, 100, 78, 1, 211, 104, 202, 8, 122, 53, 182, 150, 143, 149, 108, 255, 147]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![29, 65, 103, 104, 170, 57, 56, 14, 65, 95, 121, 72, 91, 157, 46, 249, 43, 28, 161, 148, 220, 137, 170, 98, 239, 28, 153, 140, 236, 190, 253, 206, 158, 92, 121, 74, 234, 245, 0, 109, 31, 14, 210, 182, 124, 10, 232, 54, 224, 73, 26, 118, 223, 144, 63, 169, 106, 250, 36, 133, 224, 213, 132, 55, 57, 23, 30, 214, 67, 60, 90, 173, 77, 16]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![166, 208, 108, 58, 218, 203, 22, 94, 157, 98, 240, 251, 130, 243, 247, 118, 176, 138, 51, 103, 66, 172, 185, 116, 57, 186, 1, 135, 231, 238, 141, 179, 198, 18, 9, 126, 56, 146, 112, 110, 119, 188, 110, 97, 69, 115, 215, 140, 223, 110, 145, 50, 183, 156, 15, 41, 158, 149, 129, 187, 156, 214, 97, 130, 38, 44, 226, 155, 64, 244, 182, 72, 85, 113]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![100, 75, 51, 68, 157, 71, 122, 51, 71, 132, 81, 242, 178, 41, 163, 39, 180, 65, 246, 88, 188, 203, 42, 183, 152, 43, 186, 143, 205, 158, 197, 39, 152, 11, 76, 33, 238, 247, 68, 95, 83, 24, 228, 149, 120, 94, 129, 127, 141, 149, 246, 214, 192, 170, 94, 102, 241, 40, 17, 137, 243, 36, 103, 222, 207, 81, 107, 181, 182, 109, 28, 21, 74, 38]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![239, 146, 145, 92, 54, 235, 175, 3, 22, 42, 169, 156, 187, 1, 156, 67, 98, 136, 117, 159, 54, 194, 143, 100, 40, 178, 32, 47, 142, 125, 183, 8, 255, 51, 231, 138, 99, 109, 236, 61, 66, 230, 239, 121, 199, 145, 63, 240, 139, 90, 230, 198, 100, 140, 121, 19, 82, 224, 213, 58, 160, 28, 71, 150, 85, 176, 103, 158, 120, 161, 240, 165, 169, 104]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![153, 250, 80, 160, 231, 215, 92, 208, 127, 240, 82, 238, 80, 213, 50, 167, 155, 159, 46, 248, 0, 30, 240, 14, 8, 74, 89, 25, 235, 89, 206, 79, 60, 215, 221, 215, 147, 145, 65, 248, 166, 68, 99, 123, 147, 0, 80, 223, 189, 123, 82, 33, 66, 28, 79, 6, 41, 225, 108, 22, 165, 232, 160, 178, 158, 171, 57, 156, 122, 79, 199, 33, 173, 51, 83]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![167, 225, 159, 89, 25, 128, 248, 126, 112, 125, 226, 241, 172, 146, 10, 96, 180, 157, 171, 141, 78, 209, 59, 234, 229, 212, 145, 104, 72, 0, 142, 80, 108, 25, 23, 171, 15, 192, 116, 1, 198, 21, 102, 11, 138, 247, 200, 197, 95, 38, 242, 222, 5, 151, 131, 29, 167, 219, 86, 80, 109, 9, 146, 109, 251, 27, 236, 40, 12, 198, 174, 134, 172, 213, 22]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![45, 108, 105, 255, 195, 24, 90, 160, 227, 149, 61, 71, 128, 111, 152, 25, 90, 193, 32, 128, 84, 52, 36, 141, 11, 120, 251, 13, 141, 239, 193, 24, 152, 2, 61, 253, 84, 67, 49, 5, 213, 12, 158, 116, 70, 185, 83, 139, 43, 127, 31, 42, 136, 45, 143, 169, 254, 148, 210, 192, 7, 20, 77, 175, 10, 226, 249, 193, 89, 130, 123, 120, 143, 59, 192]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![44, 85, 35, 166, 88, 106, 206, 213, 185, 244, 255, 209, 152, 32, 130, 44, 149, 33, 18, 49, 38, 20, 145, 240, 128, 68, 209, 120, 79, 169, 129, 141, 64, 56, 124, 112, 200, 240, 131, 189, 162, 122, 157, 175, 1, 76, 177, 69, 171, 57, 77, 127, 83, 77, 140, 206, 243, 185, 40, 250, 157, 84, 123, 100, 142, 102, 69, 115, 183, 9, 22, 227, 126, 164, 246]),
            (DeltaKey { hash: address[1], key_type: Delta(1) }, vec![81, 42, 145, 211, 144, 238, 91, 176, 218, 94, 92, 33, 190, 103, 21, 227, 55, 48, 218, 27, 239, 71, 243, 78, 242, 8, 188, 58, 16, 64, 148, 26, 246, 202, 153, 252, 222, 217, 236, 18, 64, 253, 242, 223, 246, 101, 29, 207, 233, 36, 200, 213, 250, 160, 119, 180, 244, 51, 255, 49, 171, 142, 174, 2, 37, 86, 51, 6, 31, 73, 142, 49, 101, 144, 165]),
            (DeltaKey { hash: address[2], key_type: State }, vec![8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8]),
        ];

        for (key, data) in stuff {
            DATABASE.lock().expect("Database mutex is poison").force_update(&key, &data).unwrap();
        }
        address
    }
}