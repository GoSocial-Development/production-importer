# Production Importer

## Setup and running
 1. `cargo build --release`
 2. Create a `config.ini` file - look at `config.ini.example` for more info.
 4. Run the executable.

## INI Configuration
1. `CONFIG`:
    - `SQL_HOST` - SQL Host
    - `SQL_PORT` - SQL Port
    - `SQL_USER` - SQL Username
    - `SQL_PASS` - SQL Password
    - `SQL_DB` - SQL DataBase
    - `SQL_PREFIX` - SQL DataBase Prefix
    - `SQL_TABLE` - SQL Table from where to import
    - `ELASTIC_HOST` - ElasticSearch Host
    - `ELASTIC_USER` - ElasticSearch Username
    - `ELASTIC_PASS` - ElasticSearch Password
    - `ELASTIC_INDEX` - ElasticSearch Index where to import
    - `ROWS_AT_ONCE` - Nr of rows imported at once
    - `ES_INDEX_CREATED` - yes/no if no it creates the es index (deleting any existing one)
    - `LAST_ROW` - Config.FROM_TABLE last primary key inserted
    - `THREADS` - #Threads used for row processing, everything else is single threaded

## Requirements

 `OpenSSL 1.1.1f`
 `ElasticSearch@^6.8`