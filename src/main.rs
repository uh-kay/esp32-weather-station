mod wifi;
mod structs;

use anyhow::Result;
use embedded_graphics::{mono_font::{ascii::FONT_6X10, MonoTextStyle}, pixelcolor::Rgb565, prelude::{Point, RgbColor}, text::Text};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{adc::{attenuation::DB_2_5, oneshot::{config::AdcChannelConfig, AdcChannelDriver, AdcDriver}}, delay::Delay, gpio::{self, Level, PinDriver}, i2c::{config::Config, I2cDriver}, prelude::Peripherals, spi::{SpiDeviceDriver, SpiDriver, SpiDriverConfig}},
    mqtt::client::{EspMqttClient, EventPayload, MqttClientConfiguration, QoS},
};
use log::{error, info};
use bme680::*;
use serde::Serialize;
use wifi::{try_reconnect_wifi, wifi};
use std::{thread, time::Duration};
use structs::{Config as MqttConfig, MqttMessage};
use st7735_lcd::Orientation;
use embedded_graphics_core::draw_target::DrawTarget;
use embedded_graphics::Drawable;

const MAX_RETRY_ATTEMPTS: u32 = 3;
const RETRY_DELAY_MS: u64 = 5000;

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let mut delay: Delay = Default::default();
    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;

    // BME680 pins
    let sda = peripherals.pins.gpio22;
    let scl = peripherals.pins.gpio23;

    // Rain sensor pins
    let d0 = PinDriver::input(peripherals.pins.gpio19)?;
    let mut power = PinDriver::output(peripherals.pins.gpio21)?;

    // GUVA-S12SD pins
    let adc = AdcDriver::new(peripherals.adc1)?;
    let adc_config = AdcChannelConfig {
        calibration: true,
        attenuation: DB_2_5,
        ..Default::default()
    };
    let mut adc_pin = AdcChannelDriver::new(&adc, peripherals.pins.gpio35, &adc_config).unwrap();

    // ST7735 LCD pins
    let dc = PinDriver::output(peripherals.pins.gpio27)?;
    let sclk = peripherals.pins.gpio18;
    let mosi = peripherals.pins.gpio5;
    let cs = peripherals.pins.gpio26;
    let rst = PinDriver::output(peripherals.pins.gpio4)?;

    // SPI config
    let spi_driver = SpiDriver::new(peripherals.spi2, sclk, mosi, None::<gpio::AnyIOPin>, &SpiDriverConfig::new())?;
    let spi = SpiDeviceDriver::new(spi_driver, Some(cs), &esp_idf_svc::hal::spi::config::Config::new())?;

    let mut display = st7735_lcd::ST7735::new(spi, dc, rst, false, false, 128, 128);
    display.init(&mut delay).unwrap();
    display.set_orientation(&Orientation::PortraitSwapped).unwrap();
    display.set_offset(0, 0);

    // I2C config
    let i2c_config = Config::new();

    let mqtt_config = MqttConfig::new();

    // Initialize I2C and BME680
    let i2c = I2cDriver::new(peripherals.i2c0, sda, scl, &i2c_config)?;
    let mut dev = Bme680::init(i2c, &mut delay, I2CAddress::Secondary)
        .map_err(|e| {
            error!("Error at bme680 init {e:?}");
            anyhow::anyhow!("BME680 initialization failed: {:?}", e)
        })?;

    // BME680 sensor settings
    let settings = SettingsBuilder::new()
        .with_humidity_oversampling(OversamplingSetting::OS2x) // note: humidity is constant because there is a bug in the library
        .with_pressure_oversampling(OversamplingSetting::OS4x)
        .with_temperature_oversampling(OversamplingSetting::OS8x)
        .with_temperature_filter(IIRFilterSize::Size3)
        .with_gas_measurement(Duration::from_millis(1500), 320, 25)
        .with_temperature_offset(-2.2)
        .with_run_gas(true)
        .build();

    let profile_dur = dev.get_profile_dur(&settings.0)
        .map_err(|e| anyhow::anyhow!("Failed to get profile duration: {:?}", e))?;
    info!("Profile duration {:?}", profile_dur);

    dev.set_sensor_settings(&mut delay, settings)
        .map_err(|e| anyhow::anyhow!("Failed to apply sensor settings: {:?}", e))?;

    dev.set_sensor_mode(&mut delay, PowerMode::ForcedMode)
        .map_err(|e| anyhow::anyhow!("Failed to set sensor mode: {:?}", e))?;

    let sensor_settings = dev.get_sensor_settings(settings.1);
    info!("Sensor settings: {:?}", sensor_settings);

    // Initialize WiFi
    let mut wifi = wifi(&mqtt_config.ssid, &mqtt_config.password, peripherals.modem, sysloop)?;

    // Create MQTT client configuration
    let mqtt_client_config = MqttClientConfiguration {
        client_id: Some(&mqtt_config.client_id),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        server_certificate: Some(mqtt_config.server_cert),
        client_certificate: Some(mqtt_config.client_cert),
        private_key: Some(mqtt_config.private_key),
        ..Default::default()
    };

    // Create MQTT client with retry logic
    let mut retry_count = 0;
    let mut client = None;

    while retry_count < MAX_RETRY_ATTEMPTS {
        match EspMqttClient::new_cb(
            &mqtt_config.mqtts_url,
            &mqtt_client_config,
            move |message_event| {
                match message_event.payload() {
                    EventPayload::Connected(_) => info!("Connected"),
                    EventPayload::Subscribed(id) => info!("Subscribed to id: {}", id),
                    EventPayload::Received { data, .. } => {
                        if !data.is_empty() {
                            let mqtt_message: Result<MqttMessage, serde_json::Error> =
                                serde_json::from_slice(data);

                            match mqtt_message {
                                Ok(message) => {
                                    info!("Received: {:?}", message);
                                }
                                Err(err) => error!(
                                    "Could not parse message: {:?}. Err: {}",
                                    std::str::from_utf8(data).unwrap(),
                                    err
                                ),
                            }
                        }
                    }
                    _ => info!("{:?}", message_event.payload()),
                };
            },
        ) {
            Ok(mqtt_client) => {
                client = Some(mqtt_client);
                break;
            }
            Err(e) => {
                error!("Failed to create MQTT client (attempt {}): {:?}", retry_count + 1, e);
                retry_count += 1;
                thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
            }
        }
    }

    let mut client = client.ok_or_else(|| anyhow::anyhow!("Failed to create MQTT client after {} attempts", MAX_RETRY_ATTEMPTS))?;

    // Subscribe to MQTT topic with retry logic
    retry_count = 0;
    while retry_count < MAX_RETRY_ATTEMPTS {
        match client.subscribe(&mqtt_config.sub_topic, QoS::AtLeastOnce) {
            Ok(_) => {
                info!("Successfully subscribed to topic");
                break;
            }
            Err(e) => {
                error!("Failed to subscribe (attempt {}): {:?}", retry_count + 1, e);
                retry_count += 1;
                thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
            }
        }
    }

    info!("Starting main loop");

    loop {
        delay.delay_ms(30000u32);

        // Check wifi connection and try reconnecting
        if !wifi.is_connected()? {
            try_reconnect_wifi(&mut wifi, &mut client, &mqtt_config)?;
            continue;
        }

        // Getting data from BME680
        dev.set_sensor_mode(&mut delay, PowerMode::ForcedMode)
            .map_err(|e| {
                error!("Unable to set sensor mode: {:?}", e);
                anyhow::anyhow!("Failed to set sensor mode: {:?}", e)
            })?;

        let (data, _state) = dev.get_sensor_data(&mut delay)
            .map_err(|e| {
                error!("Unable to get sensor data: {:?}", e);
                anyhow::anyhow!("Failed to get sensor data: {:?}", e)
            })?;

        // Getting data from UV sensor
        let sensor_value = adc.read_raw(&mut adc_pin)?;
        let voltage = (sensor_value as f32 / 4095.0) * 3.3;
        let uv_index = voltage * 10.0;

        // Getting data from rain sensor
        power.set_high()?;
        delay.delay_ms(10);
        let rain_state = d0.get_level();
        power.set_low()?;

        #[derive(Serialize)]
        struct SensorData {
            temperature: u32,
            humidity: u32,
            pressure: u32,
            gas_resistance: u32,
            rain: bool,
            uv_index: u16,
        }

        let sensor_data = SensorData {
            temperature: data.temperature_celsius() as u32,
            humidity: data.humidity_percent() as u32,
            pressure: data.pressure_hpa() as u32,
            gas_resistance: data.gas_resistance_ohm() as u32,
            rain: if Level::High == rain_state {
                false
            } else {
                true
            },
            uv_index: uv_index as u16,
        };

        // Format the sensor data into json
        let sensor_json = serde_json::to_string(&sensor_data)?;

        // Clear the display to avoid overlap
        display.clear(Rgb565::BLACK).unwrap();
        let style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

        // Display sensor value to LCD screen
        let sensor_data_str = format!("Temperature: {}\nHumidity: {}\nAir Pressure: {}\nGas Resistance: \n{}\nRaining: {}\nUV Index: {}", sensor_data.temperature, sensor_data.humidity, sensor_data.pressure, sensor_data.gas_resistance, sensor_data.rain, sensor_data.uv_index);
        Text::new(&sensor_data_str, Point::new(10, 40), style).draw(&mut display).unwrap();

        // Publish the json payload
        match client.publish(
            &mqtt_config.pub_topic,
            QoS::AtLeastOnce,
            false,
            sensor_json.as_bytes(),
        ) {
            Ok(_) => info!("Successfully published sensor data"),
            Err(e) => {
                error!("Failed to publish sensor data: {:?}", e);
                // Attempt to reconnect on publish failure
                try_reconnect_wifi(&mut wifi, &mut client, &mqtt_config)?;
            }
        }
    }
}
