# Production Importer

## Setup and running
 1. `cargo build --release`
 2. Create a `config.ini` file - look at `config.ini.example` for more info.
 4. Run the executable.

## INI Configuration
1. `CONFIG`:
    - `SQL_HOST` * - SQL Host 
    - `SQL_PORT`* - SQL Port
    - `SQL_USER`* - SQL Username
    - `SQL_PASS`* - SQL Password
    - `SQL_DB`* - SQL DataBase
    - `SQL_PREFIX`* - SQL DataBase Prefix
    - `SQL_TABLE`* - SQL Table from where to import
    - `ELASTIC_HOST`* - ElasticSearch Host
    - `ELASTIC_USER` - ElasticSearch Username
    - `ELASTIC_PASS` - ElasticSearch Password
    - `ELASTIC_INDEX`* - ElasticSearch Index where to import
    - `ROWS_AT_ONCE`* - Nr of rows imported at once
    - `ES_INDEX_CREATED` - yes/no if no it creates the es index (deleting any existing one)
    - `LAST_ROW` - Config.FROM_TABLE last primary key inserted
    - `THREADS`* - #Threads used for row processing, everything else is single threaded
    - `DATE_FORMAT`* - Format of the date fields (uses this pattern, values 1 - 130 are accepted: [all possible values](https://learn.microsoft.com/en-us/sql/t-sql/functions/cast-and-convert-transact-sql?view=sql-server-ver16#date-and-time-styles) )
    - `ELASTIC_GEO_FIELD` - name of the geo_shape field if a geo field needs to be created, Points only
    - `SQL_LAT_FIELD` - Name of the field of the LAT value
    - `SQL_LON_FIELD` - Name of the field of the LON value
    - `CATCHALL` - List catch all fields that need to be created in this format myCombinedField=fieldA,FieldB;MyOtherCombinedField=fieldC;FieldZ
\* \- required

## Requirements

 `OpenSSL 1.1.1f`
 `ElasticSearch@^6.8`