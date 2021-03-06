// Copyright 2019. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use crate::{
    blocks::Block,
    chain_storage::{BlockHeaderAccumulatedData, ChainBlock},
    transactions::types::HashOutput,
};
use serde::{Deserialize, Serialize};

/// The representation of a historical block in the blockchain. It is essentially identical to a protocol-defined
/// block but contains some extra metadata that clients such as Block Explorers will find interesting.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct HistoricalBlock {
    /// The number of blocks that have been mined since this block, including this one. The current tip will have one
    /// confirmation.
    pub confirmations: u64,
    /// The underlying block
    pub block: Block,
    /// Accumulated data
    pub accumulated_data: BlockHeaderAccumulatedData,
}

impl HistoricalBlock {
    pub fn new(block: Block, confirmations: u64, accumulated_data: BlockHeaderAccumulatedData) -> Self {
        HistoricalBlock {
            block,
            confirmations,
            accumulated_data,
        }
    }

    pub fn confirmations(&self) -> u64 {
        self.confirmations
    }

    /// Returns a reference to the block of the HistoricalBlock
    pub fn block(&self) -> &Block {
        &self.block
    }

    pub fn hash(&self) -> &HashOutput {
        &self.accumulated_data.hash
    }

    pub fn into_block(self) -> Block {
        self.block
    }

    pub fn into_chain_block(self) -> ChainBlock {
        ChainBlock {
            accumulated_data: self.accumulated_data,
            block: self.block,
        }
    }
}

impl From<HistoricalBlock> for Block {
    fn from(block: HistoricalBlock) -> Self {
        block.block
    }
}
