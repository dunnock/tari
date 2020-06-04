#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RpcRequest {
    #[prost(uint32, tag = "1")]
    pub request_id: u32,
    #[prost(uint32, tag = "2")]
    pub method: u32,
    #[prost(bytes, tag = "4")]
    pub message: std::vec::Vec<u8>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RpcResponse {
    #[prost(uint32, tag = "1")]
    pub request_id: u32,
    #[prost(uint32, tag = "2")]
    pub method: u32,
    #[prost(uint32, tag = "3")]
    pub status: u32,
    #[prost(string, tag = "4")]
    pub error: std::string::String,
    #[prost(bytes, tag = "5")]
    pub message: std::vec::Vec<u8>,
}
