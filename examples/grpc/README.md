# Unary gRPC example

This service returns the same protobuf message that it receives. Request and response use the same message type, so the PHP handler can return the raw bytes.

Install `grpcurl` to call the service through reflection. Rapira parses the included schema at startup.

Start Rapira from the repository root:

```powershell
rapira serve --config examples/grpc/rapira.toml
```

List services:

```powershell
grpcurl -plaintext 127.0.0.1:9001 list
```

Call the echo method:

```powershell
grpcurl -plaintext -d '{"text":"hello"}' 127.0.0.1:9001 example.v1.Echo/Echo
```

The response is `{"text":"hello"}`. Add `-v` to show response headers and trailers. These client commands get their schemas from reflection.

An application that changes message fields needs the `google/protobuf` PHP package or a matching protobuf extension. It can generate PHP classes with `protoc --php_out` during its build. Decode `$call->getMessage()` with the generated request class. Pass the serialized response to `$call->respond()`.
