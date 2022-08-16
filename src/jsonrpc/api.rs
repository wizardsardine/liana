use crate::{
    jsonrpc::{Error, Request, Response},
    DaemonControl,
};

/// Handle an incoming JSONRPC2 request.
pub fn handle_request(control: &DaemonControl, req: Request) -> Result<Response, Error> {
    let result = match req.method.as_str() {
        "getinfo" => serde_json::json!(&control.get_info()),
        "getnewaddress" => serde_json::json!(&control.get_new_address()),
        "listcoins" => serde_json::json!(&control.list_coins()),
        "stop" => serde_json::json!({}),
        _ => {
            return Err(Error::method_not_found());
        }
    };

    Ok(Response::success(req.id, result))
}
