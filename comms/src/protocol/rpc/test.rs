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

use crate::{
    peer_manager::NodeId,
    protocol::rpc::{service::RequestId, RpcError},
};
use async_trait::async_trait;
use bytes::Bytes;
use log::*;
use std::sync::Arc;

#[test]
fn todo() {}

//--------------------------------------------------  --------------------------------------------------

pub struct TestServiceImpl {
    greeting: String,
}

#[async_trait]
impl RpcService for TestServiceImpl {
    async fn call(&self, req: RawRequest) -> Result<RawResponse, RpcStatus> {
        match req.method().unwrap() {
            TestServiceMethods::SayHello => {
                let resp = self.say_hello(req.decode()?).await?;
                Ok(resp.encode())
            },
            TestServiceMethods::Error => {
                let resp = self.error(req.decode()?).await?;
                Ok(resp.encode())
            },
        }
    }
}

#[derive(EnumIter)]
pub enum TestServiceMethods {
    SayHello = 0,
    Error = 1,
}

pub trait ToEnum: Sized {
    fn from_value(value: u32) -> Option<Self>;
}

impl<T: IntoEnumIterator> ToEnum for T {
    fn from_value(value: u32) -> Option<Self> {
        Self::iter().nth(value as usize)
    }
}

// impl TryFrom<MethodId> for TestServiceMethods {
//     type Error = RpcError;
//
//     fn try_from(value: u32) -> Result<Self, Self::Error> {
//         match value {
//             0 => Ok(TestServiceMethods::SayHello),
//             1 => Ok(TestServiceMethods::Error),
//             _ => Err(RpcError::Todo),
//         }
//     }
// }

#[derive(prost::Message)]
pub struct SayHelloRequest {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(uint32, tag = "2")]
    age: u32,
}

#[derive(prost::Message)]
pub struct SayHelloResponse {
    #[prost(string, tag = "1")]
    greeting: String,
}

#[async_trait]
impl TestService for TestServiceImpl {
    async fn say_hello(&self, request: Request<SayHelloRequest>) -> Result<Response<SayHelloResponse>, RpcStatus> {
        let greeting = format!("{} {}, {}", self.greeting, request.message.name, request.message.age);
        Ok(Response::new(SayHelloResponse { greeting }))
    }

    async fn error(&self, request: Request<()>) -> Result<Response<()>, RpcStatus> {
        Err(RpcStatus::not_implemented("error method is not implemented"))
    }
}

#[async_trait]
pub trait TestService {
    async fn say_hello(&self, request: Request<SayHelloRequest>) -> Result<Response<SayHelloResponse>, RpcStatus>;

    async fn error(&self, request: Request<()>) -> Result<Response<()>, RpcStatus>;
}

//--------------------------------------------------  --------------------------------------------------

use crate::{
    framing::CanonicalFraming,
    message::MessageExt,
    proto::rpc::{RpcRequest, RpcResponse},
};
use futures::{
    future::BoxFuture,
    stream::{Fuse, FuturesUnordered},
    AsyncRead,
    AsyncWrite,
    FutureExt,
    SinkExt,
    StreamExt,
};
use prost::Message;
use std::{fmt, fmt::Display};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

const LOG_TARGET: &str = "comms::rpc";

pub struct Request<T> {
    request_id: RequestId,
    pub message: T,
    pub peer: NodeId,
}

pub struct Response<T> {
    pub message: T,
}

impl<T: prost::Message> Response<T> {
    pub fn new(message: T) -> Self {
        Self { message }
    }

    pub fn encode(&self) -> RawResponse {
        RawResponse {
            payload: self.message.to_encoded_bytes().into(),
        }
    }
}

pub type MethodId = u32;

pub struct RawRequest {
    request_id: RequestId,
    method: MethodId,
    payload: Bytes,
    peer: NodeId,
}

impl RawRequest {
    pub fn decode<T: prost::Message + Default>(mut self) -> Result<Request<T>, RpcError> {
        let message = T::decode(&mut self.payload)?;
        Ok(Request {
            request_id: self.request_id,
            message,
            peer: self.peer,
        })
    }
}

impl RawRequest {
    pub fn method<T: ToEnum>(&self) -> Option<T> {
        T::from_value(self.method)
    }
}

pub struct RawResponse {
    payload: Bytes,
}

#[async_trait]
pub trait RpcService: Send + Sync + 'static {
    async fn call(&self, req: RawRequest) -> Result<RawResponse, RpcStatus>;
}

pub struct Rpc<TSocket, TService> {
    framed: Fuse<CanonicalFraming<TSocket>>,
    service: Arc<TService>,
    request_id: u32,
    peer: NodeId,
    queue: FuturesUnordered<BoxFuture<'static, Result<RpcResponse, RpcError>>>,
}

impl<TSocket, TService> Rpc<TSocket, TService>
where
    TSocket: AsyncRead + AsyncWrite + Unpin,
    TService: RpcService,
{
    pub fn new(framed: CanonicalFraming<TSocket>, service: TService, peer: NodeId) -> Self {
        Self {
            framed: framed.fuse(),
            service: Arc::new(service),
            request_id: 0,
            peer,
            queue: FuturesUnordered::new(),
        }
    }

    pub async fn start(mut self) -> Result<(), RpcError> {
        loop {
            futures::select! {
                msg = self.framed.select_next_some() => {
                    self.handle_message(msg?.freeze()).await;
                },

                reply = self.queue.select_next_some() => {
                    self.handle_reply(reply).await?;
                },
            }
        }
        Ok(())
    }

    async fn handle_message(&self, mut msg: Bytes) {
        let mut service = self.service.clone();
        let peer = self.peer.clone();
        self.queue.push(
            async move {
                let req = RpcRequest::decode(&mut msg)?;
                let method = req.method;
                let request_id = req.request_id;
                let req = RawRequest {
                    request_id,
                    method,
                    payload: req.message.into(),
                    peer,
                };

                match service.call(req).await {
                    Ok(resp) => Ok(RpcResponse {
                        request_id,
                        method,
                        status: RpcStatus::ok().as_code(),
                        error: Default::default(),
                        message: resp.payload.to_vec(),
                    }),
                    Err(status) => Ok(RpcResponse {
                        request_id,
                        method,
                        status: status.as_code(),
                        error: status.to_string(),
                        message: Default::default(),
                    }),
                }
            }
            .boxed(),
        );
    }

    async fn handle_reply(&mut self, reply_result: Result<RpcResponse, RpcError>) -> Result<(), RpcError> {
        let reply = match reply_result {
            Ok(resp) => resp,
            Err(err) => {
                debug!(
                    target: LOG_TARGET,
                    "Failed to handle request from peer `{}`: {}",
                    self.peer.short_str(),
                    err
                );
                // TODO: This should shut down the whole service
                self.framed.close().await?;
                return Ok(());
            },
        };
        self.framed.send(reply.to_encoded_bytes().into()).await?;
        Ok(())
    }

    fn next_request_id(&mut self) -> u32 {
        let next_id = self.request_id;
        match self.request_id.checked_add(1) {
            Some(next_id) => {
                self.request_id = next_id;
            },
            None => {
                // TODO: log this
                self.request_id = 0;
            },
        }

        next_id
    }
}

#[derive(Debug)]
pub struct RpcStatus {
    code: RpcStatusCode,
    details: String,
}

impl RpcStatus {
    pub fn ok() -> Self {
        RpcStatus {
            code: RpcStatusCode::Ok,
            details: Default::default(),
        }
    }

    pub fn invalid_method<T: ToString>(details: T) -> Self {
        RpcStatus {
            code: RpcStatusCode::InvalidMethod,
            details: details.to_string(),
        }
    }

    pub fn not_implemented<T: ToString>(details: T) -> Self {
        RpcStatus {
            code: RpcStatusCode::NotImplemented,
            details: details.to_string(),
        }
    }

    pub fn bad_request<T: ToString>(details: T) -> Self {
        Self {
            code: RpcStatusCode::BadRequest,
            details: details.to_string(),
        }
    }

    pub fn general<T: ToString>(details: T) -> Self {
        Self {
            code: RpcStatusCode::General,
            details: details.to_string(),
        }
    }

    fn as_code(&self) -> u32 {
        self.code as u32
    }
}

impl Display for RpcStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.details)
    }
}

impl From<RpcError> for RpcStatus {
    fn from(err: RpcError) -> Self {
        match err {
            RpcError::DecodeError(_) => Self::bad_request("Failed to decode request"),
            err => {
                debug!(target: LOG_TARGET, "{}", err);
                Self::general("Request failed")
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RpcStatusCode {
    Ok = 200,
    InvalidMethod = 300,
    BadRequest = 400,
    NotImplemented = 404,
    General = 500,
}
