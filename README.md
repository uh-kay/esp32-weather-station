# ESP32 Weather Station

This project is an ESP32-based weather station that measures temperature, humidity, pressure, UV index, and detects rain, using BME680 sensor, GUVA-S12SD sensor, and rain sensor. It also a 128x128 ST7735 LCD display for displaying the data. The weather station connects to a WiFi network and publishes weather data to AWS IoT Core MQTT broker.

## Structure of the Code

The project is structured as follows:

- The `src` directory contains the main code for the weather station.
  - The `main.rs` file is the entry point of the application.
  - The `wifi.rs` file contains functions for managing WiFi connectivity.
  - The `structs.rs` file defines the data structures used in the project.
- The `build.rs` file sets up the build environment for the project.
- The `Cargo.toml` file specifies the dependencies required for the project.
- The `aws` directory contains the AWS certificates used in the project.
- The `.env` file contains all the environment variables required for the project.

## Running the Weather Station

To run the weather station, follow these steps:

1. Connect the ESP32 board to your computer via USB.
2. Download and put the AWS certificates (root CA1, private key, and device certs) in `/aws`.
3. Connect to the WiFi network with the SSID and password then connect to AWS url specified in the `.env`.
3. Install the required dependencies by running `cargo build` in the project directory.
4. Flash the firmware to the ESP32 board using `cargo run --release`.
6. The weather station will connect to AWS IoT Core and start publishing weather data.

Make sure to update the WiFi SSID, WiFi password, MQTT broker URL, client ID, topic, and other configuration values in the `.env` file according to your setup. See `.env-examples` for examples on configuring `.env`.

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.