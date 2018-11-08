#![no_std]

/// Enigma runtime implementation

#[macro_use]
extern crate sgx_tstd as std;
extern crate sgx_types;
#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate rmp_serde as rmps;
extern crate enigma_tools_t;
extern crate json_patch;
extern crate wasmi;
extern crate hexutil;

use wasmi::{MemoryRef, RuntimeArgs, RuntimeValue, Trap, Externals};
use std::vec::Vec;
use std::string::ToString;
use enigma_tools_t::common::errors_t::EnclaveError;
use std::str;

pub mod data;
pub mod ocalls_t;
pub mod eng_resolver;
use data::{ContractState, StatePatch, DeltasInterface, IOInterface};

#[derive(Debug, Clone)]
pub struct RuntimeResult{
    pub state_delta: Option<StatePatch>,
    pub updated_state: Option<ContractState>,
    pub result: Vec<u8>,
}

pub struct Runtime {
    memory: MemoryRef,
    args: Vec<u8>,
    result: RuntimeResult,
    init_state: ContractState,
    current_state: ContractState,
}

impl Runtime {

    pub fn new(memory: MemoryRef, args: Vec<u8>, contract_id: [u8; 32]) -> Runtime {
        let init_state = ContractState::new( contract_id.clone() );
        let current_state = ContractState::new(contract_id);
        let result = RuntimeResult{ result: Vec::new(), state_delta: None, updated_state: None };

        Runtime { memory, args, result, init_state, current_state }
    }

    pub fn new_with_state(memory: MemoryRef, args: Vec<u8>, state: ContractState) -> Runtime{
        let init_state = state.clone();
        let current_state = state;
        let result = RuntimeResult{ result: Vec::new(), state_delta: None, updated_state: None };

        Runtime { memory, args, result, init_state, current_state }
    }

    /// args:
    /// * `value` - value holder: the start address of value in memory
    /// * `value_len` - the length of value holder
    ///
    /// Copy memory starting address 0 of length 'value_len' to `value` and to `self.result.result`
    pub fn from_memory(&mut self, args: RuntimeArgs) -> Result<(), EnclaveError> {
        let value: u32 = args.nth_checked(0).unwrap();
        let value_len: i32 = args.nth_checked(1).unwrap();

        let mut buf = Vec::with_capacity(value_len as usize);
        for _ in 0..value_len{
            buf.push(0);
        }

        match self.memory.get_into(0, &mut buf[..]) {
            Ok( () ) => {
                match self.memory.set(value, &buf[..]) {
                    Ok( () ) => {
                        self.result.result = match self.memory.get(0, value_len as usize) {
                            Ok(v) => v,
                            Err(e) => return Err(EnclaveError::ExecutionErr{code: "ret code".to_string(), err: e.to_string()}),
                        };
                    },
                    Err(e) => return Err(EnclaveError::ExecutionErr{code: "memory".to_string(), err: e.to_string()}),
                }
                Ok(())
            },
            Err(e) => return Err(EnclaveError::ExecutionErr{code: "memory".to_string(), err: e.to_string()}),
        }
    }

    /// args:
    /// * `key` - the start address of key in memory
    /// * `key_len` - the length of key
    ///
    /// Read `key` from the memory, then read from the state the value under the `key`
    /// and copy it to the memory at address 0.
    pub fn read_state (&mut self, args: RuntimeArgs) -> Result<i32, EnclaveError> {
        let key = args.nth_checked(0);
        let key_len: u32 = args.nth_checked(1).unwrap();
        let mut buf = Vec::with_capacity(key_len as usize);
        for _ in 0..key_len{
            buf.push(0);
        }
        match self.memory.get_into(key.unwrap(), &mut buf[..]) {
            Ok( () ) => (),
            Err(e) => return Err(EnclaveError::ExecutionErr{code: "read state".to_string(), err: e.to_string()}),
        }
        let key1 = str::from_utf8(&buf)?;
        let value_vec = serde_json::to_vec(&self.current_state.json[key1]).expect("Failed converting Value to vec in Runtime while reading state");
        self.memory.set(0, &value_vec).unwrap(); // TODO: Impl From so we could use `?`
        Ok( value_vec.len() as i32 )

    }

    /// args:
    /// * `key` - the start address of key in memory
    /// * `key_len` - the length of the key
    /// * `value` - the start address of value in memory
    /// * `value_len` - the length of the value
    ///
    /// Read `key` and `value` from memory, and write (key, value) pair to the state
    pub fn write_state (&mut self, args: RuntimeArgs) -> Result<(), EnclaveError>{
        let key = args.nth_checked(0);
        let key_len: u32 = args.nth_checked(1).unwrap();
        let value: u32 = args.nth_checked(2).unwrap();
        let value_len: u32 = args.nth_checked(3).unwrap();

        let mut buf = Vec::with_capacity(key_len as usize);
        for _ in 0..key_len {
            buf.push(0);
        }

        match self.memory.get_into(key.unwrap(), &mut buf[..]){
            Ok(v) => v,
            Err(e) => return Err(EnclaveError::ExecutionErr{code: "write state".to_string(), err: e.to_string()}),
        }

        let mut val = Vec::with_capacity(value_len as usize);
        for _ in 0..value_len {
            val.push(0);
        }

        match self.memory.get_into(value, &mut val[..]){
            Ok(v) => v,
            Err(e) => return Err(EnclaveError::ExecutionErr{code: "write state".to_string(), err: e.to_string()}),
        }

        let key1 = str::from_utf8(&buf)?;
        let value: serde_json::Value = serde_json::from_slice(&val).expect("Failed converting into Value while writing state in Runtime");
        self.current_state.write_key(key1, &value).unwrap();
        Ok(())
    }

    /// args:
    /// * `ptr` - the start address in memory
    /// * `len` - the length
    ///
    /// Copy the memory of length `len` starting at address `ptr` to `self.result.result`
    pub fn ret(&mut self, args: RuntimeArgs) -> Result<(), EnclaveError> {
        let ptr: u32 = args.nth_checked(0)?;
        let len: u32 = args.nth_checked(1)?;

        self.result.result = match self.memory.get(ptr, len as usize){
            Ok(v)=>v,
            Err(e)=>return Err(EnclaveError::ExecutionErr{code: "Error in getting value from runtime memory".to_string(), err: e.to_string()}),
        };
        Ok(())
    }

    /// Destroy the runtime, returning currently recorded result of the execution
    pub fn into_result(mut self) -> /*Vec<u8>*/Result<RuntimeResult, EnclaveError> {
        //self.result.result.to_owned()
        self.result.state_delta =
            match self.current_state.generate_delta(Some(&self.init_state), None){
                Ok(v) => Some(v),
                Err(e) => return Err(EnclaveError::ExecutionErr{code: "Error in generating state delta".to_string(), err: e.to_string()}),
            };

        self.result.updated_state = Some(self.current_state);
        Ok(self.result.clone())
    }

    pub fn eprint(&mut self, args: RuntimeArgs) -> Result<(), EnclaveError> {
        let msg_ptr: u32 = args.nth_checked(0)?;
        let msg_len: u32 = args.nth_checked(1)?;
        match self.memory.get(msg_ptr, msg_len as usize) {
            Ok(res) => {
                let st = str::from_utf8(&res)?;
                println!("PRINT: {}", st);

            },
            Err(e) => return Err(EnclaveError::ExecutionErr{code: "Error in Logging debug".to_string(), err: e.to_string()}),
        }
        Ok(())
    }


}

impl Externals for Runtime {

    fn invoke_index(&mut self, index: usize, args: RuntimeArgs) -> Result<Option<RuntimeValue>, Trap> {
        match index {
            eng_resolver::ids::RET_FUNC => {
                &mut Runtime::ret(self, args);
                Ok(None)
            }
            eng_resolver::ids::WRITE_STATE_FUNC => {
                &mut Runtime::write_state(self, args);
                Ok(None)
            }
            eng_resolver::ids::READ_STATE_FUNC => {
                Ok(Some(RuntimeValue::I32(Runtime::read_state(self, args).unwrap())))
            }
            eng_resolver::ids::FROM_MEM_FUNC => {
                &mut Runtime::from_memory(self, args);
                Ok(None)
            }

            eng_resolver::ids::EPRINT_FUNC => {
                &mut Runtime::eprint(self, args);
                Ok(None)
            }
            _ => unimplemented!("Unimplemented function at {}", index),
        }
    }
}