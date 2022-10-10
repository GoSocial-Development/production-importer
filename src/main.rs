extern crate ini;
use anyhow::{Result};
use chrono::NaiveDate;
use colored::Colorize;
use ini::{Ini, Properties};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::prelude::*;
use std::io::{self, BufRead};
use std::ops::{Add, Div, Mul, Sub};
use std::path::Path;
use std::process;
use std::time::Instant;
use tiberius::{error::Error, AuthMethod, Client, Config, Query, Row};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ini_path = "./config.ini";
    validate_config(ini_path);
    let ini = Ini::load_from_file(ini_path).unwrap();
    let config: &Properties = ini.section(Some("CONFIG")).unwrap();

    let rows_at_once = config
        .get("ROWS_AT_ONCE")
        .expect("ROWS_AT_ONCE config missing")
        .to_string()
        .parse::<f64>()
        .expect("ROWS_AT_ONCE config empty");

    let es_auth: HashMap<String, String> = get_elastic_auth(config);

    let mut last_row = match config.get("LAST_ROW") {
        Some(v) => v.parse::<i32>().unwrap(),
        None => {
            update_last_row(0, ini_path);
            0
        }
    };

    let mut rows_imported: f64 = 0 as f64;

    let mut client = mssql_connect(config).await.unwrap();
    println!("{}", "Connected to MSSQL".green());

    let primary_key = get_primary_key(config).await.unwrap();
    println!("{}", "Grabbed Primary Table Key".green());

    let mapping_rows = get_mapping(config).await.unwrap();
    let mapping = create_mapping(mapping_rows).unwrap();
    println!("{}", "Grabbed Mapping Rows".green());

    let total_rows_nr = get_total_nr_rows(config, &primary_key, &last_row)
        .await
        .unwrap();
    println!("{}", "Grabbed Total Row Count".green());

    if config.get("ES_INDEX_CREATED").unwrap_or("no") != "yes" {
        let res = elastic_create_index(
            [
                config.get("ELASTIC_HOST").unwrap().to_string(),
                "/".to_string(),
                config.get("ELASTIC_INDEX").unwrap().to_string(),
            ]
            .join(""),
            mapping.to_string(),
            &es_auth,
        )
        .await;

        if !res {
            println!(
                "{} {}",
                "Cannot create elastic index: ".red(),
                config.get("ELASTIC_HOST").unwrap()
            );
            process::exit(1);
        }
        println!("{}", "Created ES Index".green());
        mark_index_created(ini_path);
    } else {
        println!("{}", "Skipped ES Index Creation".green());
    }

    let mut fields = Vec::new();
    let mut fields_select = Vec::new();
    for (field, x) in mapping["mappings"]["data"]["properties"]
        .as_object()
        .unwrap()
        .iter()
    {
        let mut field_data = HashMap::new();
        field_data.insert("name".to_string(), field.to_string());
        field_data.insert("type".to_string(), x["type"].to_string());
        if field != "the_geom"
            && field != "id_catchall"
            && field != "name_catchall"
            && x["type"].as_str().unwrap() != "date"
        {
            fields_select.push(field.to_string());
            fields.push(field_data);
        } else if x["type"].as_str().unwrap() == "date" {
            fields.push(field_data);
            fields_select.push(
                [
                    "convert(varchar, ",
                    &field.to_string(),
                    ", 23) as ",
                    &field.to_string(),
                ]
                .join(""),
            );
        }
    }

    loop {
        let query_now = Instant::now();
        let select = Query::new(
            [
                "SELECT TOP ",
                &rows_at_once.to_string(),
                " ",
                &fields_select.join(","),
                " from ",
                config.get("DB").unwrap(),
                ".",
                config.get("PREFIX").unwrap(),
                ".",
                config.get("FROM_TABLE").unwrap(),
                " where ",
                &primary_key,
                " > ",
                &last_row.to_string(),
                "  ORDER BY ",
                &primary_key,
                " asc",
            ]
            .join("")
            .to_string(),
        );

        let stream = select.query(&mut client).await?;

        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(_) => {
                println!("{}", "NO MORE ROWS".blink());
                break;
            }
        };
        let last_entry: i32 = rows.last().unwrap().get(&*primary_key).unwrap();
        let query_duration = query_now.elapsed();
        let rows_process_now = Instant::now();
        rows_imported = rows_imported.add(rows.len() as f64);
        let body = build_elastic_body(
            rows,
            &fields,
            config.get("ELASTIC_INDEX").unwrap().to_string(),
        );
        let rows_process_duration = rows_process_now.elapsed();
        let insert_now = Instant::now();
        match insert_elastic(
            body,
            config.get("ELASTIC_HOST").unwrap().to_string(),
            &es_auth,
        )
        .await
        {
            Ok(_) => {
                last_row = last_entry;
            }
            Err(e) => {
                println!("{}  {:?}", "Error inserting data into Elastic:".red(), e);
                process::exit(1);
            }
        }

        let remaining_nr_rows = total_rows_nr.sub(rows_imported);
        let time_per_row = (query_now.elapsed().as_millis() as f64).div(rows_at_once);
        let time_remaining = remaining_nr_rows.mul(time_per_row).div(3600000 as f64);
        print!("{}[2J", 27 as char);
        print!("{esc}[2J{esc}[1;1H", esc = 27 as char);
        println!(
            "Imported: {} | Query: {:.2?}; Row Process Took {:.2?}; Insert Took {:.2?}; Total: {:.2?} | Time remaining {:.1?}h",
            rows_imported.to_string().green(),
            rows_process_duration,
            query_duration,
            insert_now.elapsed(),
            query_now.elapsed(),
            time_remaining
        );
        update_last_row(last_row, ini_path);
    }

    Ok(())
}

async fn mssql_connect(config: &Properties) -> Result<tiberius::Client<Compat<TcpStream>>, Error> {
    let mut mssql_config = Config::new();

    mssql_config.host(config.get("HOST").unwrap());
    mssql_config.port(
        config
            .get("PORT")
            .unwrap()
            .to_string()
            .parse::<u16>()
            .unwrap(),
    );
    mssql_config.database(config.get("DB").unwrap());
    mssql_config.authentication(AuthMethod::sql_server(
        config.get("USER").unwrap(),
        config.get("PASSWORD").unwrap(),
    ));
    mssql_config.trust_cert();

    let tcp = TcpStream::connect(mssql_config.get_addr()).await?;
    tcp.set_nodelay(true)?;

    let client = match Client::connect(mssql_config, tcp.compat_write()).await {
        // Connection successful.
        Ok(client) => client,
        // The server wants us to redirect to a different address
        Err(Error::Routing { host, port }) => {
            let mut mssql_config = Config::new();

            mssql_config.host(&host);
            mssql_config.port(port);
            mssql_config.database(config.get("DB").unwrap());
            mssql_config.authentication(AuthMethod::sql_server(
                config.get("USER").unwrap(),
                config.get("PASSWORD").unwrap(),
            ));

            let tcp = TcpStream::connect(mssql_config.get_addr()).await?;
            tcp.set_nodelay(true)?;

            // we should not have more than one redirect, so we'll short-circuit here.
            Client::connect(mssql_config, tcp.compat_write()).await?
        }
        Err(e) => {
            println!("{} :{}", "Error connecting to DB".red(), e);
            process::exit(1);
        }
    };
    Ok(client)
}

fn mark_index_created(ini_path: &str) {
    let mut new_lines: Vec<String> = Vec::new();
    let mut added = false;
    if let Ok(lines) = read_lines(ini_path) {
        for line in lines {
            if let Ok(ip) = line {
                if ip.contains("ES_INDEX_CREATED") {
                    added = true;
                    new_lines.push("ES_INDEX_CREATED=yes".to_string());
                } else {
                    new_lines.push(ip);
                }
            }
        }
        if !added {
            new_lines.push("ES_INDEX_CREATED=yes".to_string());
        }
        match fs::write(ini_path, new_lines.join("\n")) {
            Ok(_) => true,
            Err(e) => {
                println!("Could not update INI last row => {:?}", e);
                process::exit(1);
            }
        };
    }
}

fn build_elastic_body(
    rows: Vec<Row>,
    fields: &Vec<HashMap<String, String>>,
    index: String,
) -> String {
    let body = process_rows(rows, fields, index);
    [body.join(""), "\n".to_string()].join("")
}

fn process_rows(
    rows: Vec<Row>,
    fields: &Vec<HashMap<String, String>>,
    index: String,
) -> Vec<String> {
    let mut body: Vec<String> = Vec::new();
    for row in rows {
        let mut es_row = json!({});
        body.push(
            [
                json!({"index": {
                    "_index": index,
                    "_type": "data"
                }})
                .to_string(),
                "\n".to_string(),
            ]
            .join(""),
        );
        for e in row.into_iter().enumerate() {
            let value = match e.1 {
                tiberius::ColumnData::I32(value) => match value {
                    Some(v) => Value::from(v),
                    None => Value::from(0 as i32),
                },
                tiberius::ColumnData::I64(value) => match value {
                    Some(v) => Value::from(v),
                    None => Value::from(0 as i64),
                },
                tiberius::ColumnData::F32(value) => match value {
                    Some(v) => Value::from(v),
                    None => Value::from(0 as f32),
                },
                tiberius::ColumnData::F64(value) => match value {
                    Some(v) => Value::from(v),
                    None => Value::from(0 as f64),
                },
                tiberius::ColumnData::Numeric(value) => match value {
                    Some(v) => match v.to_string().parse::<f32>() {
                        Ok(v) => Value::from(v),
                        Err(_) => Value::from(0.0 as f32),
                    },
                    None => Value::from(0 as i32),
                },
                tiberius::ColumnData::String(value) => match value {
                    Some(v) => Value::from(v),
                    None => Value::from(""),
                },
                unkown => Value::from(""),
            };

            let field_data = &fields[e.0];
            if field_data.get("type").unwrap() == "\"date\"" {
                match NaiveDate::parse_from_str(&value.to_string().replace("\"", ""), "%Y-%m-%d") {
                    Ok(_) => {
                        es_row[field_data.get("name").unwrap()] = value;
                    }
                    _=>{}
                }
            } else {
                es_row[field_data.get("name").unwrap()] = value;
            }
        }
        es_row["the_geom"] = json!({
            "type": "point",
            "coordinates": [
                es_row["locLongWGS84_wh"],
                es_row["locLatWGS84_wh"]
            ]
        });
        es_row["id_catchall"] = Value::from(
            [
                es_row["eOwnerID"].to_string(),
                es_row["eNormalizedID"].to_string(),
                es_row["wprdOperIDHartStandard"].to_string(),
            ]
            .join(","),
        );
        es_row["name_catchall"] = Value::from(
            [
                es_row["wlaAPIHartStandard"].to_string(),
                es_row["wlaWellNum"].to_string(),
                es_row["wlaWellName"].to_string(),
                es_row["wlaLeaseName"].to_string(),
                es_row["wlaLeaseNum"].to_string(),
            ]
            .join(" "),
        );

        body.push([json!(es_row).to_string(), "\n".to_string()].join(""));
    }
    body
}

async fn insert_elastic(
    body: String,
    elastic_host: String,
    auth: &HashMap<String, String>,
) -> Result<bool, anyhow::Error> {
    let client = reqwest::Client::new();
    match client
        .put(
            [
                elastic_host,
                String::from("/_bulk?wait_for_active_shards=0"),
            ]
            .join(""),
        )
        .body(body)
        .basic_auth(
            auth.get("username").unwrap_or(&"".to_string()),
            Some(auth.get("password").unwrap_or(&"".to_string())),
        )
        .header("content-type", "application/json")
        .send()
        .await
    {
        reqwest::Result::Ok(e) => {
            let status = e.status().to_string();
            let text = e.text().await.unwrap();
            if text.contains("FORBIDDEN/12/index read-only") {
                return Err(anyhow::anyhow!(
                    "ElasticSearch Index is READ ONLY, Stopping Import"
                ));
            }
            let json: Value = serde_json::from_str(&text).unwrap();
            if json["errors"].as_bool().unwrap_or(false) {
                let mut errors: i32 = 0;
                for item in json["items"].as_array() {
                    if item.first().unwrap()["index"]["status"] == 400 {
                        let mut file = OpenOptions::new()
                            .write(true)
                            .append(true)
                            .open("./errors")
                            .unwrap();

                        if let Err(e) = writeln!(file, "{}", item.first().unwrap().to_string()) {
                            println!("Couldn't write to file: {}", e);
                        }
                        errors = errors + 1;
                    }
                }

                if errors > 0 {
                    println!("{} {} {}", "Could not insert ".red(), errors, "rows".red());
                }
            }
            if status == "200 OK" {
                return Ok(true);
            }
            Err(anyhow::anyhow!(
                "ElasticSearch responded with an unknown status: {}",
                status
            ))
        }
        reqwest::Result::Err(e) => Err(anyhow::anyhow!("Request Error {}", e)),
    }
}

fn create_mapping(rows: Vec<Row>) -> Result<Value, anyhow::Error> {
    let mut mapping = json!({
        "settings": {
            "index": {
                "refresh_interval": "1s",
                "number_of_shards": 28,
                "number_of_replicas": 0
            },
            "analysis": {
                "analyzer": {
                    "lowercase_analyzer": {
                        "filter": [
                            "lowercase"
                        ],
                        "type": "custom",
                        "tokenizer": "keyword"
                    }
                }
            }
        },
        "mappings":{
            "data":{
                "properties":{
                    "the_geom":{
                        "type": "geo_shape"
                    },
                    "id_catchall":{
                        "type": "text",
                        "fields": {
                          "keyword": {
                            "type": "keyword"
                          }
                        },
                        "analyzer": "lowercase_analyzer"
                    },
                    "name_catchall":{
                        "type": "text",
                        "fields": {
                          "keyword": {
                            "type": "keyword"
                          }
                        },
                        "analyzer": "lowercase_analyzer"
                    }
                }
            }
        }
    });
    for row in rows {
        let field = row
            .try_get::<&str, _>("COLUMN_NAME")?
            .ok_or_else(|| anyhow::anyhow!("Unexpected null"))?
            .to_string();
        let field_type = row
            .try_get::<&str, _>("DATA_TYPE")?
            .ok_or_else(|| anyhow::anyhow!("Unexpected null"))?
            .to_string();

        if field_type == "varchar" || field_type == "ntext" || field_type == "nvarchar" {
            mapping["mappings"]["data"]["properties"][field] = json!({
              "type": "text",
              "fields": {
                "keyword": {
                  "type": "keyword"
                }
              },
              "analyzer": "lowercase_analyzer"
            });
        } else if field_type == "float" {
            mapping["mappings"]["data"]["properties"][field] = json!({
              "type": "double"
            });
        } else if field_type == "bigint" {
            mapping["mappings"]["data"]["properties"][field] = json!({
              "type": "long"
            });
        } else if field_type == "int" {
            mapping["mappings"]["data"]["properties"][field] = json!({
              "type": "integer"
            });
        } else if field_type == "numeric" {
            mapping["mappings"]["data"]["properties"][field] = json!({
              "type": "integer"
            });
        } else if field_type == "date" || field_type == "datetime2" {
            mapping["mappings"]["data"]["properties"][field] = json!({
              "type": "date"
            });
        } else {
            println!("{} {}", "Ignored field tipe:".yellow(), field_type);
        }
    }

    Ok(mapping)
}

async fn elastic_create_index(url: String, data: String, auth: &HashMap<String, String>) -> bool {
    let client = reqwest::Client::new();
    match client.delete(&url).send().await {
        reqwest::Result::Ok(_) => true,
        reqwest::Result::Err(_) => true,
    };

    match client
        .put(url)
        .body(data)
        .basic_auth(
            auth.get("username").unwrap(),
            Some(auth.get("password").unwrap()),
        )
        .header("content-type", "application/json")
        .send()
        .await
    {
        reqwest::Result::Ok(e) => {
            if e.status().to_string() == "200 OK" {
                return true;
            }
            false
        }
        reqwest::Result::Err(_) => false,
    }
}

fn update_last_row(last_row: i32, ini_path: &str) {
    let mut added = false;
    let mut new_lines: Vec<String> = Vec::new();
    if let Ok(lines) = read_lines(ini_path) {
        // Consumes the iterator, returns an (Optional) String
        for line in lines {
            if let Ok(ip) = line {
                if ip.contains("LAST_ROW") {
                    added = true;
                    new_lines.push(["LAST_ROW=", &last_row.to_string()].join(""));
                } else {
                    new_lines.push(ip);
                }
            }
        }
        if !added {
            new_lines.push(["LAST_ROW=", &last_row.to_string()].join(""));
        }
        match fs::write(ini_path, new_lines.join("\n")) {
            Ok(_) => true,
            Err(e) => {
                println!("{} {:?}", "Could not update INI last row =>".red(), e);
                process::exit(1);
            }
        };
    }
}

fn read_lines<P>(filename: P) -> io::Result<io::Lines<io::BufReader<File>>>
where
    P: AsRef<Path>,
{
    let file = File::open(filename)?;
    Ok(io::BufReader::new(file).lines())
}

fn get_elastic_auth(config: &Properties) -> HashMap<String, String> {
    let mut es_auth: HashMap<String, String> = HashMap::new();
    let es_username = config.get("ELASTIC_USER").unwrap_or("").to_string();
    let es_password = config.get("ELASTIC_PASS").unwrap_or("").to_string();
    es_auth.insert("username".to_string(), es_username.to_string());
    es_auth.insert("password".to_string(), es_password.to_string());

    es_auth
}

fn validate_config(ini_path: &str) {
    let ini: Ini = Ini::load_from_file(ini_path).expect("Cannot open ini file:");
    let config: &Properties = ini
        .section(Some("CONFIG"))
        .expect("CONFIG section missing from INI");
    for entry in [
        "HOST",
        "PORT",
        "USER",
        "PASSWORD",
        "DB",
        "PREFIX",
        "FROM_TABLE",
        "ELASTIC_HOST",
        "ELASTIC_USER",
        "ELASTIC_PASS",
        "ELASTIC_INDEX",
        "ROWS_AT_ONCE",
    ] {
        config.get(entry).or_else(|| {
            println!("{} {}", "Missing DB Config for".red(), entry);
            process::exit(1);
        });
    }
}

async fn get_primary_key(config: &Properties) -> Result<String, anyhow::Error> {
    let mut client = mssql_connect(config).await.unwrap();
    let primary_query = Query::new([
        "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE WHERE OBJECTPROPERTY(OBJECT_ID(CONSTRAINT_SCHEMA + '.' + QUOTENAME(CONSTRAINT_NAME)), 'IsPrimaryKey') = 1 AND TABLE_SCHEMA ='",
        config.get("PREFIX").unwrap(),
        "'",
        " AND TABLE_CATALOG = '",
        config.get("DB").unwrap(),
        "' AND TABLE_NAME ='",
        config.get("FROM_TABLE").unwrap(),
        "'"
    ].join("").to_string());

    let primary_stream = primary_query.query(&mut client).await?;
    let primary_row = primary_stream.into_row().await?;

    let primary_key = primary_row
        .unwrap()
        .try_get::<&str, _>("COLUMN_NAME")?
        .ok_or_else(|| anyhow::anyhow!("Unexpected null"))?
        .to_string();
    Ok(primary_key)
}

async fn get_mapping(config: &Properties) -> Result<Vec<Row>, anyhow::Error> {
    let mut client = mssql_connect(config).await.unwrap();
    let mapping_select = Query::new(
        [
            "SELECT DATA_TYPE , COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA ='",
            config.get("PREFIX").unwrap(),
            "'",
            " AND TABLE_CATALOG = '",
            config.get("DB").unwrap(),
            "' AND TABLE_NAME ='",
            config.get("FROM_TABLE").unwrap(),
            "'",
        ]
        .join("")
        .to_string(),
    );
    let mapping_stream = mapping_select.query(&mut client).await?;
    let mapping_rows = mapping_stream.into_first_result().await?;
    Ok(mapping_rows)
}

async fn get_total_nr_rows(
    config: &Properties,
    primary_key: &str,
    last_row: &i32,
) -> Result<f64, anyhow::Error> {
    let mut client = mssql_connect(config).await.unwrap();
    let select = Query::new(
        [
            "SELECT count(*) as  total from ",
            config.get("DB").unwrap(),
            ".",
            config.get("PREFIX").unwrap(),
            ".",
            config.get("FROM_TABLE").unwrap(),
            " where ",
            &primary_key,
            " > ",
            &last_row.to_string(),
        ]
        .join("")
        .to_string(),
    );
    let stream = select.query(&mut client).await?;
    let total_row = stream.into_row().await?;
    Ok(total_row.unwrap().get::<i32, _>("total").unwrap() as f64)
}
