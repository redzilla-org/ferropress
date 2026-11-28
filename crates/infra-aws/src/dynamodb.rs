//! `DynamoDB` over its JSON protocol (`x-amz-json-1.0`).

use serde_json::{Map, Value, json};

use crate::http::{Call, Reply};
use crate::sigv4::PathStyle;
use crate::{AwsClient, AwsError, Endpoint};

/// An item in `DynamoDB` JSON: attribute name to typed value (`{"S": "…"}`, `{"N": "…"}`).
pub type Item = Map<String, Value>;

/// Outcome of a version-gated write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// The item was written.
    Applied,
    /// The stored item already has this or a newer version; nothing changed.
    Stale,
}

/// One page of a query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// Items in key order (newest first for the descending queries issued here).
    pub items: Vec<Item>,
    /// Pass back as `start` to read the next page; `None` on the last page.
    pub next: Option<Item>,
}

/// A `DynamoDB` client for one endpoint.
#[derive(Clone, Debug)]
pub struct DynamoDb {
    aws: AwsClient,
    endpoint: Endpoint,
}

impl DynamoDb {
    /// A client reaching `endpoint`.
    #[must_use]
    pub fn new(aws: AwsClient, endpoint: Endpoint) -> Self {
        Self { aws, endpoint }
    }

    /// The public regional endpoint `https://dynamodb.<region>.amazonaws.com`.
    #[must_use]
    pub fn regional_endpoint(region: &str) -> Endpoint {
        Endpoint::https(&format!("dynamodb.{region}.amazonaws.com"))
    }

    /// `GetItem` by full primary key (eventually consistent, which the read path accepts).
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails or the reply is malformed.
    pub fn get_item(&self, table: &str, key: &Item) -> Result<Option<Item>, AwsError> {
        const OP: &str = "DynamoDB.GetItem";
        let reply = self.call(OP, "GetItem", &json!({ "TableName": table, "Key": key }))?;
        match object(OP, &reply)?.remove("Item") {
            None => Ok(None),
            Some(Value::Object(item)) => Ok(Some(item)),
            Some(_) => Err(malformed(OP, "Item is not a map")),
        }
    }

    /// `PutItem` only if the stored item has no `version` or an older one
    /// (`attribute_not_exists(version) OR version < :new`). `version` is written into the item.
    /// Versions compare as strings, so they must sort chronologically
    /// (`<post_modified_gmt>#<outbox id>`).
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails for any reason other than the condition.
    pub fn put_item_versioned(
        &self,
        table: &str,
        mut item: Item,
        version: &str,
    ) -> Result<Write, AwsError> {
        const OP: &str = "DynamoDB.PutItem";
        item.insert("version".to_owned(), json!({ "S": version }));
        let request = json!({
            "TableName": table,
            "Item": item,
            "ConditionExpression": "attribute_not_exists(#v) OR #v < :new",
            "ExpressionAttributeNames": { "#v": "version" },
            "ExpressionAttributeValues": { ":new": { "S": version } },
        });
        let reply = self.send(OP, "PutItem", &request)?;
        if reply.status == 400 && crate::http::error_code(&reply.body) == CONDITION_FAILED {
            return Ok(Write::Stale);
        }
        reply.success(OP)?;
        Ok(Write::Applied)
    }

    /// `DeleteItem` by full primary key; deleting a missing item succeeds.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails.
    pub fn delete_item(&self, table: &str, key: &Item) -> Result<(), AwsError> {
        self.call(
            "DynamoDB.DeleteItem",
            "DeleteItem",
            &json!({ "TableName": table, "Key": key }),
        )
        .map(drop)
    }

    /// `Query` for the items whose `pk` equals `pk`, sort key descending, at most `limit` per
    /// page, continuing after `start` when given.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails or the reply is malformed.
    pub fn query_descending(
        &self,
        table: &str,
        pk: &str,
        limit: u32,
        start: Option<Item>,
    ) -> Result<Page, AwsError> {
        const OP: &str = "DynamoDB.Query";
        let mut request = json!({
            "TableName": table,
            "KeyConditionExpression": "pk = :pk",
            "ExpressionAttributeValues": { ":pk": { "S": pk } },
            "ScanIndexForward": false,
            "Limit": limit,
        });
        if let (Some(start), Value::Object(map)) = (start, &mut request) {
            map.insert("ExclusiveStartKey".to_owned(), Value::Object(start));
        }
        page(OP, &self.call(OP, "Query", &request)?)
    }

    /// Unconditional `PutItem`: replaces any item with the same key.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails.
    pub fn put_item(&self, table: &str, item: &Item) -> Result<(), AwsError> {
        self.call(
            "DynamoDB.PutItem",
            "PutItem",
            &json!({ "TableName": table, "Item": item }),
        )
        .map(drop)
    }

    /// `UpdateItem` with `update` (an update expression using `names` and `values`), returning
    /// the updated attributes (`ReturnValues: UPDATED_NEW`).
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails or the reply is malformed.
    pub fn update_item(
        &self,
        table: &str,
        key: &Item,
        update: &str,
        names: &Value,
        values: &Value,
    ) -> Result<Item, AwsError> {
        const OP: &str = "DynamoDB.UpdateItem";
        let request = json!({
            "TableName": table,
            "Key": key,
            "UpdateExpression": update,
            "ExpressionAttributeNames": names,
            "ExpressionAttributeValues": values,
            "ReturnValues": "UPDATED_NEW",
        });
        match object(OP, &self.call(OP, "UpdateItem", &request)?)?.remove("Attributes") {
            Some(Value::Object(attributes)) => Ok(attributes),
            _ => Err(malformed(OP, "Attributes is missing")),
        }
    }

    /// `Query` on the global secondary index `index` for items whose `attribute` equals
    /// `value` (a typed value such as `{"N": "7"}`), continuing after `start` when given.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails or the reply is malformed.
    pub fn query_index(
        &self,
        table: &str,
        index: &str,
        attribute: &str,
        value: &Value,
        start: Option<Item>,
    ) -> Result<Page, AwsError> {
        const OP: &str = "DynamoDB.Query";
        let mut request = json!({
            "TableName": table,
            "IndexName": index,
            "KeyConditionExpression": "#k = :v",
            "ExpressionAttributeNames": { "#k": attribute },
            "ExpressionAttributeValues": { ":v": value },
        });
        if let (Some(start), Value::Object(map)) = (start, &mut request) {
            map.insert("ExclusiveStartKey".to_owned(), Value::Object(start));
        }
        page(OP, &self.call(OP, "Query", &request)?)
    }

    fn call(&self, operation: &'static str, target: &str, body: &Value) -> Result<Reply, AwsError> {
        self.send(operation, target, body)?.success(operation)
    }

    fn send(&self, operation: &'static str, target: &str, body: &Value) -> Result<Reply, AwsError> {
        let target = format!("DynamoDB_20120810.{target}");
        let body = body.to_string();
        self.aws.send(&Call {
            operation,
            service: "dynamodb",
            region: None,
            path_style: PathStyle::Standard,
            endpoint: &self.endpoint,
            method: "POST",
            path: "/",
            query: &[],
            headers: &[
                ("content-type", "application/x-amz-json-1.0"),
                ("x-amz-target", &target),
            ],
            body: body.as_bytes(),
        })
    }
}

const CONDITION_FAILED: &str = "ConditionalCheckFailedException";

fn object(operation: &'static str, reply: &Reply) -> Result<Map<String, Value>, AwsError> {
    match serde_json::from_slice(&reply.body) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(malformed(operation, "body is not a JSON object")),
    }
}

// Every Query reply (table or index) carries `Items` and an optional `LastEvaluatedKey`.
fn page(operation: &'static str, reply: &Reply) -> Result<Page, AwsError> {
    let mut reply = object(operation, reply)?;
    let items = match reply.remove("Items") {
        Some(Value::Array(items)) => items
            .into_iter()
            .map(|item| match item {
                Value::Object(item) => Ok(item),
                _ => Err(malformed(operation, "an item is not a map")),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(malformed(operation, "Items is missing")),
    };
    let next = match reply.remove("LastEvaluatedKey") {
        None => None,
        Some(Value::Object(key)) => Some(key),
        Some(_) => return Err(malformed(operation, "LastEvaluatedKey is not a map")),
    };
    Ok(Page { items, next })
}

const fn malformed(operation: &'static str, what: &'static str) -> AwsError {
    AwsError::Malformed { operation, what }
}
