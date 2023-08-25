use std::u32;

use poem::{listener::TcpListener, Result, Route};
use poem_openapi::{
    param::Path,
    param::Query,
    payload::{Json, PlainText},
    ApiRequest, ApiResponse, Object, OpenApi, OpenApiService,
};

#[derive(Object)]
struct Pet {
    id: String,
    name: String,
}

#[derive(ApiRequest)]
enum CreatePet {
    /// This request receives a pet in JSON format(application/json).
    CreateByJSON(Json<Pet>),
    /// This request receives a pet in text format(text/plain).
    CreateByPlainText(PlainText<String>),
}

#[derive(Object)]
struct CoverCalibrator {
    device_number: u32,
    max_brightness: u32,
    brightness: u32,
}

#[derive(ApiResponse)]
enum GetBrightnessResponse {
    #[oai(status = 200)]
    /// This is the response for the bightness inquiery
    CoverCalibrator(Json<CoverCalibrator>),
}
struct Api;

#[OpenApi]
impl Api {
    #[oai(path = "/covercalibrator/:device_number/brightness", method = "get")]
    async fn get_brightness(
        &self,
        device_number: Path<Option<u32>>,
        ClientID: Query<Option<u32>>,
        ClientTransactionID: Query<Option<u32>>,
    ) -> Result<GetBrightnessResponse> {
        let device_number = device_number.unwrap_or(0);
        let client_id = ClientID.unwrap_or(1);
        let client_transaction_id = ClientTransactionID.unwrap_or(1234);
        println!(
            "{} : {} : {}",
            device_number, client_id, client_transaction_id
        );

        Ok(GetBrightnessResponse::CoverCalibrator(Json(
            CoverCalibrator {
                device_number,
                max_brightness: 255,
                brightness: 128,
            },
        )))
    }
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let api_service =
        OpenApiService::new(Api, "Hello World", "1.0").server("http://localhost:3000/api");
    let ui = api_service.swagger_ui();
    let app = Route::new().nest("/api", api_service).nest("/", ui);

    poem::Server::new(TcpListener::bind("127.0.0.1:3000"))
        .run(app)
        .await
}
