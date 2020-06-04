//  Copyright 2020, The Tari Project
//
//  Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
//  following conditions are met:
//
//  1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
//  disclaimer.
//
//  2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
//  following disclaimer in the documentation and/or other materials provided with the distribution.
//
//  3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
//  products derived from this software without specific prior written permission.
//
//  THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
//  INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
//  DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
//  SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
//  SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
//  WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
//  USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

#[cfg(test)]
mod test;

mod error;
pub use error::RpcError;

mod service;
use service::RpcService;

use crate::protocol::ProtocolId;
use std::collections::HashMap;

const DEFAULT_MAX_CONCURRENCY: usize = 256;

pub struct Rpc {
    services: HashMap<ProtocolId, Box<dyn RpcService>>,
    max_concurrency: usize,
}

impl Rpc {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn add_service<T: RpcService>(&mut self, protocol: ProtocolId, service: T) -> &mut Self {
        self.services.insert(protocol, Box::new(service));
        self
    }

    pub fn run(mut self) {}
}

impl Default for Rpc {
    fn default() -> Self {
        Self {
            services: HashMap::new(),
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
        }
    }
}
