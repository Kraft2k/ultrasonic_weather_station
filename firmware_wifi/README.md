# Wi-Fi firmware for the weather station

An ESP-IDF build for ESP32-S3 Super Mini. It reuses the Modbus register map
from `../firmware/src/weather.rs`, uploads all ten weather metrics over MQTT/TLS with
QoS 1 and waits for PUBACK before deleting each queued value. A sample cycle
runs every 20 minutes; the ESP32 deep-sleeps between cycles. The weather station
itself remains on its separate 10–30 V supply.

## First start and registration

The station is being transferred to `basis / vostochnoe / first_pear_nursery`.
The AFC transfer pipeline keeps the old MQTT scope active while the new
firmware is tested. Its target dashboard uses
`ultrasonic-weather-station-basis-full.json` in the AFC template directory:

```sh
/home/kraft/afc/scripts/weather_station_pipeline.sh prepare-transfer \
  --sensor-id ultrasonic_weather_station_01 \
  --from-tenant kraftrobotics --from-region s_posad --from-fleet 189 \
  --to-tenant basis --to-region vostochnoe --to-fleet first_pear_nursery \
  --grafana-user basis-first-pear-nursery \
  --firmware-dir /home/kraft/ultrasonic_weather_station/firmware_wifi \
  --dashboard-template ultrasonic-weather-station-basis-full.json
```

Review the dry run first, then add `--apply`. The broker update requires sudo
on the public VPS. After flashing, verify fresh readings in the target scope
before using `finalize-transfer`; choose explicitly whether to move or delete
the old tenant's history. Do not copy another device's MQTT credentials.
Without a saved Wi-Fi profile the ESP32 opens `Weather-Setup`
(password `weather-setup`); connect and browse to the address printed in the
serial monitor. Save Wi-Fi and coordinates once. The credentials are stored in
NVS and remain available after deep sleep. Coordinates can be zero for an
indoor station; they are metadata and do not affect measurements.

The full 17-panel Grafana dashboard compares `node_id=sensor` with
`node_id=open-meteo` inside the same `basis / vostochnoe / first_pear_nursery`
scope. AFC reads the station coordinates from its registry; each Wi-Fi upload
updates them, and the Open-Meteo worker refreshes the model when they change.

## Две сети Wi-Fi для перевозки станции

В локальном `device.env` укажите `WIFI_SSID`, `WIFI_PASSWORD`,
`WIFI_LATITUDE`, `WIFI_LONGITUDE` для первой сети и `WIFI_SSID_2`,
`WIFI_PASSWORD_2`, `WIFI_LATITUDE_2`, `WIFI_LONGITUDE_2` для второй.
Координаты второй сети обязательны, если указан её SSID. После изменения этих полей
обязательно задайте **новый** `WIFI_PROFILE_ID`: прошивка установит оба профиля
в NVS при первом запуске новой версии. Если в старой NVS у второй сети нет
координат, она не используется до установки нового профиля. Пароли в журнал
не выводятся.

При каждой отправке станция пробует последнюю успешно использованную сеть,
затем вторую. После успешного подключения запоминает её номер в NVS и добавляет
к каждой отправляемой метрике координаты этой сети. Если обе
сети недоступны, показания остаются в очереди NVS; через следующий 20-минутный
цикл попытка повторится. Профиль, введённый через `Weather-Setup`, заменяет
первую сеть и сохраняет вторую. При переносе станции достаточно включить питание
в зоне действия одной из этих двух сетей. Для модели с постоянным питанием
собирайте прошивку с `--features mains-powered`.

## Measurement cycle

1. Read holding registers 500–513 from Modbus address 1 at 4800 baud.
2. Queue the measurement in NVS so a temporary network outage does not discard it.
3. Connect to Wi-Fi, synchronize the clock, and publish each metric to the
   registered `/metric/<name>` topic over verified MQTTS.
4. Remove each queued metric only after its QoS 1 PUBACK.
5. Enter deep sleep for 1200 seconds, including after a failed upload.

No sensor power pin is switched by this firmware. Keep the Renke station on
its required external supply and use the existing GPIO5/6 UART and GPIO13
RS-485 direction wiring.

## Build and flash

Use the ESP-IDF Rust environment already installed for the sibling project:

```sh
cd firmware_wifi
source /home/kraft/export-esp.sh
unset PYTHONPATH
export IDF_PATH=/home/kraft/wifi_meteo/.embuild/espressif/esp-idf/v5.2.3
export IDF_TOOLS_PATH=/home/kraft/wifi_meteo/.embuild/espressif
export IDF_PYTHON_ENV_PATH=/home/kraft/wifi_meteo/.embuild/espressif/python_env/idf5.2_py3.10_env
export ESP_IDF_TOOLS_INSTALL_DIR=custom:/home/kraft/wifi_meteo/.embuild/espressif
export MCU=esp32s3
cargo +esp run --release

```

The first boot prints the setup AP address. After initial provisioning, the
serial monitor reports each Modbus snapshot and MQTT acknowledgement. The
original USB-serial firmware remains in `../firmware` for continuous bench
polling.

## Вариант с питанием от сети

Для ESP32 и станции с постоянным питанием через преобразователь используйте
режим сборки `mains-powered`. Он сохраняет настройку Wi-Fi, десять метрик,
MQTT/TLS, QoS 1 и очередь NVS исходной прошивки, но не вызывает глубокий сон
и не управляет ключами питания. GPIO13 нужен для направления RS-485 (DE и /RE),
его подключение сохраняется. Питание станции и трансивера постоянно включено.

Первое измерение выполняется через 30 секунд после запуска с настроенным Wi-Fi, следующие —
каждые 1200 секунд от начала цикла, включая время чтения и отправки.
Между циклами задача ждёт таймер, ESP32 остаётся включённым. Если отправка заняла
больше 20 минут, пропущенные интервалы пропускаются без серии частых опросов.
Ошибка чтения не останавливает дальнейшую работу: сохранённая очередь отправляется,
новая попытка чтения выполняется через 30 секунд, до успешного опроса. После
успешного опроса восстанавливается обычный интервал. При ошибке сети данные остаются в NVS
в пределах ёмкости существующей очереди (64 измерения).

11 ноября 2026 при холодном запуске ESP32-S3 с USB–UART WCH `1a86:55d3`
три запроса Modbus за первые 2,1 секунды завершились тайм-аутом; прежняя
прошивка затем ждала 1198 секунд до следующего измерения. Причина отсутствия
ответа датчика не установлена. Добавлен повтор через 30 секунд в режиме
`mains-powered`, а перед первым опросом добавлена задержка 30 секунд. Release
собран и записан; после полного отключения питания первый опрос через 31 секунду
дал 23,5 °C и 42,3 % влажности, а брокер подтвердил все десять метрик. Ранее
сразу после включения были переданы нули температуры и влажности, при следующем
опросе прогретой станции значения стали ненулевыми; готовность датчика является
вероятным объяснением. Сценарий повторной попытки
после тайм-аута на новой прошивке ещё требует отдельной проверки.

## Осадки не появляются в телеметрии

11 ноября 2026 года в коде очереди ESP32-S3 найден дефект: маска отправки
хранилась в одном байте при десяти метриках. После отправки первых восьми
показателей биты двух последних (`illuminance_hundreds_lux` и `rainfall_mm`)
уже не различались, поэтому они не публиковались. Чтение регистра 513 Modbus
при этом выполнялось. Маска расширена до 16 бит; формат NVS `WTH2` при загрузке
переносит ожидающие записи `WTH1` и восстанавливает отправку последних двух
показателей из сохранённых значений. Проверено сборкой release с
`--features mains-powered`; запись на плату и появление метрик в Grafana ещё
нужно проверить на подключённом устройстве. Для проверки искать в журнале
`snapshot` с `rainfall_mm`, подтверждение публикации `rainfall_mm` и запись
`metric='rainfall_mm'` в `unit_node_metrics`.

После настройки окружения из раздела выше:

```sh
cargo +esp build --offline --release --features mains-powered
# Прошивка подключённой платы и открытие монитора:
cargo +esp run --release --features mains-powered
```

Без `--features mains-powered` собирается исходный вариант с глубоким сном.
Для той же станции используется её существующий `device.env`; для нового устройства
нужна отдельная регистрация и собственные MQTT-реквизиты.

## Если запуск останавливается после `Multicore app`

Runner использует `scripts/flash.sh`, который передаёт `espflash` загрузчик
из текущей ESP-IDF сборки (а не встроенный загрузчик другой версии).
NVS не стирается. Для ручного входа в загрузчик: удерживая BOOT, нажать RESET,
отпустить RESET, затем BOOT. В настроенном окружении выполнить:

```sh
ESPFLASH_BEFORE=no-reset ESPFLASH_PORT=/dev/ttyACM0 cargo +esp run --release --features mains-powered
```

После записи нажать RESET без BOOT. Ожидаем версию загрузчика `v5.2.3`
и сообщение приложения `mains-powered mode=true`. Если остановка повторяется,
нужен полный новый журнал: совпадение версий само по себе не доказывает причину сбоя.

### USB–UART вместо встроенного USB Serial/JTAG

Для платы с flash 16 МБ и USB–UART мостом (как в `smart_greenhouse`)
выбрана консоль UART0, 115200 бод, и сброс `default-reset`.
При ошибочном выборе `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y` монитор USB–UART
показывает только ранние загрузочные строки, после чего вывод переходит
на другой интерфейс. Это не доказывает зависание приложения.
Решение перенесено из `/home/kraft/smart_greenhouse/firmware/README.md`
и `/home/kraft/ESP32_RUST_SETUP.md` (раздел 4). USB ID этой платы подтверждён:
`1a86:55d3` (QinHeng/WCH USB Single Serial).
Для платы с прямым USB Serial/JTAG потребуется соответствующая настройка консоли.
