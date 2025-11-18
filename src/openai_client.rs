use anyhow::Result;
use serde::Deserialize;

#[derive(serde::Serialize)]
struct EmbReq<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(serde::Deserialize)]
struct EmbResp {
    data: Vec<Item>,
}

#[derive(serde::Deserialize)]
struct Item {
    embedding: Vec<f32>,
}

pub async fn get_openai_embedding(text: &str, api_key: &str) -> Result<Vec<f32>> {
    let client = reqwest::Client::new();

    let resp: EmbResp = client
        .post("https://api.openai.com/v1/embeddings")
        .bearer_auth(api_key)
        .json(&EmbReq {
            model: "text-embedding-3-small",
            input: text,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(resp
        .data
        .into_iter()
        .next()
        .map(|i| i.embedding)
        .unwrap_or_default())
}

#[derive(serde::Serialize)]
struct Msg {
    role: &'static str,
    content: String,
}

#[derive(serde::Serialize)]
struct ChatReq {
    model: String,
    messages: Vec<Msg>,
}

#[derive(Deserialize)]
struct ChatResp {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMsg,
}

#[derive(Deserialize)]
struct ChoiceMsg {
    content: String,
}

pub async fn call_llm_openai(prompt: &str, model: &str, api_key: &str) -> Result<String> {
    let client = reqwest::Client::new();

    let req = ChatReq {
        model: model.to_string(),
        messages: vec![Msg {
            role: "user",
            content: prompt.to_string(),
        }],
    };

    let resp: ChatResp = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&req)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let text = resp
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .unwrap_or_default();
    Ok(text)
}
