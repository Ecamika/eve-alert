use std::{
  path::{Path, PathBuf},
  sync::Arc,
};
use time::UtcDateTime;
use tokio::{fs::File, io::AsyncReadExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileName {
  pub channel_name: String,
  pub session_started: UtcDateTime,
  pub listener_id: u64,
}

impl FileName {
  pub fn from_str(s: &str) -> Option<Self> {
    let file_name = s.replace(".txt", "");
    let mut parts = file_name.split("_").collect::<Vec<_>>();
    let listener_id = parts.pop()?;
    let session_started_time = parts.pop()?;
    let session_started_date = parts.pop()?;
    let channel_name = parts.join("_");

    let mut session_started_str = String::new();
    session_started_str.push_str(session_started_date);
    session_started_str.push_str(" ");
    session_started_str.push_str(session_started_time);

    let session_started_format =
      time::macros::format_description!("[year][month][day] [hour][minute][second]");
    let session_started = UtcDateTime::parse(&session_started_str, session_started_format).ok()?;

    Some(Self {
      channel_name,
      session_started,
      listener_id: listener_id.parse().ok()?,
    })
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChannelInfo {
  pub channel_id: String,
  pub channel_name: String,
  pub listener: String,
  pub session_started: UtcDateTime,
}

impl ChannelInfo {
  pub fn from_str(s: &str) -> Option<Self> {
    let mut channel_id = None;
    let mut channel_name = None;
    let mut listener = None;
    let mut session_started = None;

    s.split("\n").for_each(|item| {
      if item.contains("Channel ID") {
        channel_id = Some(item.replace("Channel ID:", "").trim().to_string());
      } else if item.contains("Channel Name") {
        channel_name = Some(item.replace("Channel Name:", "").trim().to_string());
      } else if item.contains("Listener") {
        listener = Some(item.replace("Listener:", "").trim().to_string());
      } else if item.contains("Session started") {
        let session_started_format =
          time::macros::format_description!("[year].[month].[day] [hour]:[minute]:[second]");
        let session_started_str = item.replace("Session started:", "").trim().to_string();
        session_started = UtcDateTime::parse(&session_started_str, session_started_format).ok();
      }
    });

    Some(Self {
      channel_name: channel_name?,
      channel_id: channel_id?,
      listener: listener?,
      session_started: session_started?,
    })
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
  pub time: UtcDateTime,
  pub sender: String,
  pub content: String,
}

impl Message {
  pub fn from_str(s: &str) -> Option<Self> {
    let mut splited = s.split(">").collect::<Vec<_>>();
    let header = splited.remove(0).trim();
    let content = splited.join(">");

    let splited = header.split(" ").collect::<Vec<_>>();
    let [_, date, time, _, sender] = splited.as_array()?;

    let format = time::macros::format_description!("[year].[month].[day] [hour]:[minute]:[second]");
    let datetime_str = format!("{date} {time}");
    let parsed = UtcDateTime::parse(&datetime_str, format).ok()?;

    Some(Self {
      content: content.trim().to_string(),
      sender: sender.to_string(),
      time: parsed,
    })
  }

  pub fn from_multi_lines(s: &str) -> Vec<Self> {
    s.split("\n")
      .filter_map(|item| Self::from_str(item))
      .collect()
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatcherSourceInfo {
  pub channel_name: String,
  pub channel_id: String,
  pub session_started: time::UtcDateTime,
  pub listener: String,
  pub listener_id: u64,
}

impl WatcherSourceInfo {
  pub fn new(path: impl AsRef<Path>, content: String) -> anyhow::Result<(Self, Vec<Message>)> {
    let file_name = path
      .as_ref()
      .file_name()
      .map(|item| item.to_str().map(|item| FileName::from_str(item)).flatten())
      .flatten()
      .ok_or(anyhow::anyhow!("can not parse file name"))?;

    let splited = content.split("\n").collect::<Vec<_>>();
    let (header, content) = splited.split_at(13);

    let header_str = header.join("\n");
    let channel_info =
      ChannelInfo::from_str(&header_str).ok_or(anyhow::anyhow!("can not parse channel info"))?;

    let content_str = content.join("\n");
    let exists_messages = Message::from_multi_lines(&content_str);

    Ok((
      Self {
        channel_name: file_name.channel_name,
        channel_id: channel_info.channel_id,
        session_started: file_name.session_started,
        listener: channel_info.listener,
        listener_id: file_name.listener_id,
      },
      exists_messages,
    ))
  }
}

#[derive(Debug)]
pub struct WatcherSource {
  pub target: File,
  pub info: Arc<WatcherSourceInfo>,
  pub target_path: PathBuf,
}

impl WatcherSource {
  pub async fn from_file(path: impl AsRef<Path>) -> anyhow::Result<(Self, Vec<Message>)> {
    let mut file = File::open(&path).await?;
    let mut content = String::new();
    file.read_to_string(&mut content).await?;

    let (info, exists_messages) = WatcherSourceInfo::new(&path, content)?;

    Ok((
      Self {
        target: file,
        info: Arc::new(info),
        target_path: path.as_ref().to_path_buf(),
      },
      exists_messages,
    ))
  }

  pub async fn update(&mut self) -> anyhow::Result<Vec<Message>> {
    let mut append_content = String::new();

    let n = self.target.read_to_string(&mut append_content).await?;

    if n > 0 {
      Ok(Message::from_multi_lines(&append_content))
    } else {
      Err(anyhow::anyhow!("content have no any news"))
    }
  }

  pub async fn close_and_delete(self) -> tokio::io::Result<()> {
    tokio::fs::remove_file(self.target_path).await?;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use time::macros::utc_datetime;

  use super::*;

  #[test]
  fn split_file_name_testing() {
    let example_without_underline = "Ecamika_20261018_111220_2124568342.txt";
    let parsed = FileName::from_str(example_without_underline);
    println!("{parsed:#?}");

    assert_eq!(
      parsed,
      Some(FileName {
        channel_name: "Ecamika".to_string(),
        listener_id: 2124568342,
        session_started: utc_datetime!(2026-10-18 11:12:20)
      })
    );

    let example_with_underline = "aaa_bbb_20261018_111220_2124568342.txt";
    let parsed = FileName::from_str(example_with_underline);
    println!("{parsed:#?}");

    assert_eq!(
      parsed,
      Some(FileName {
        channel_name: "aaa_bbb".to_string(),
        listener_id: 2124568342,
        session_started: utc_datetime!(2026-10-18 11:12:20)
      })
    )
  }

  #[test]
  fn channel_info_testing() {
    let example = r#"
        ---------------------------------------------------------------

          Channel ID:      player_bae08c3052c011ed90939abe94f5aa9b
          Channel Name:    Ecamika
          Listener:        Ecamika
          Session started: 2026.10.18 11:12:20
        ---------------------------------------------------------------
    "#;

    let parsed = ChannelInfo::from_str(example);
    println!("{parsed:#?}");

    assert_eq!(
      parsed,
      Some(ChannelInfo {
        channel_id: "player_bae08c3052c011ed90939abe94f5aa9b".to_string(),
        channel_name: "Ecamika".to_string(),
        listener: "Ecamika".to_string(),
        session_started: utc_datetime!(2026-10-18 11:12:20)
      })
    );
  }

  #[test]
  fn message_parse_testing() {
    let msg1 = "﻿[ 2026.10.19 07:13:38 ] Ecamika > 123";
    let msg2 = "﻿[ 2026.10.18 11:12:25 ] EVE系统 > 频道置顶信息：WC Escalation 凛冬社区远征交易频道【新人必看】交易教程● 交易方式卖家在本频道发布远征信息，格式：星系地点+死亡类型+剩余时间+价格，例：FH-TTC  10/10 23h 160m。买家在本频道打 1，即表示愿意接受该远征，卖家视角中先打1的买家获得该远征。买家提供共享文件夹给卖家，并支付对应价款。卖家收款后，将远征位标存入买家的共享文件夹内。旗舰远征禁止公屏暴露星系信息，仅允许说明伞内和伞外，具体信息请私聊● 交易规则1. 禁止买家无视具体远征信息盲目打1。2. 不允许二次交易，禁止倒卖到任何渠道。3. 禁止出售战斗信号（野生死亡）。4. 禁止频道内拱火、引战、政治。5.卖家没有注明价格买家有权利反悔。6. 违反上述交易规则，不能协商沟通解决问题的，联系本频道管理员 dingd xiao（QQ:2811445866）。管理员会视情况对相关人员作出限制进入频道1天、3天、7天的处罚，情节严重的永久踢出本频道。声望错误频道配置等请联系 bossboss wang (qq:304700798)。（打钱尽量备注）● 配置参考 伊斯塔 10/10 通刷所有 热90动95 (can do all escalation)金鹏   Tengu + 中级水晶套(mid-grade crystal set)旗舰    纳迦法 常在河边走 --------------------Transaction ProcessSeller provide linked location, type, remaining time and price, e,g, FH-TTC 10/10 23h 160mThe first man who type 1 in the channel get the site, buyer provide location folder and pay isk to sellerAfter seller receive the isk, seller save the location into buyer's location folderCap Escalation prohibits the exposure of system information on the public channel, and only allows the description of the parachute and outside the parachute, please ask private chatThe bitmark time must be set to permanent; otherwise, you will be banned from the channelSome recommend fittingsThe fit is the same as  above.1.Don't just random type 1 or u will be banned from this channel  2.The resale of location markers is prohibited.";

    let msg1_parsed = Message::from_str(msg1);
    let msg2_parsed = Message::from_str(msg2);

    println!("{msg1_parsed:#?}\n{msg2_parsed:#?}");

    assert_eq!(
      msg1_parsed,
      Some(Message {
        time: utc_datetime!(2026-10-19 07:13:38),
        sender: "Ecamika".to_string(),
        content: "123".to_string()
      })
    );

    assert_eq!(msg2_parsed, Some(Message {
      time: utc_datetime!(2026-10-18 11:12:25),
      sender: "EVE系统".to_string(),
      content: "频道置顶信息：WC Escalation 凛冬社区远征交易频道【新人必看】交易教程● 交易方式卖家在本频道发布远征信息，格式：星系地点+死亡类型+剩余时间+价格，例：FH-TTC  10/10 23h 160m。买家在本频道打 1，即表示愿意接受该远征，卖家视角中先打1的买家获得该远征。买家提供共享文件夹给卖家，并支付对应价款。卖家收款后，将远征位标存入买家的共享文件夹内。旗舰远征禁止公屏暴露星系信息，仅允许说明伞内和伞外，具体信息请私聊● 交易规则1. 禁止买家无视具体远征信息盲目打1。2. 不允许二次交易，禁止倒卖到任何渠道。3. 禁止出售战斗信号（野生死亡）。4. 禁止频道内拱火、引战、政治。5.卖家没有注明价格买家有权利反悔。6. 违反上述交易规则，不能协商沟通解决问题的，联系本频道管理员 dingd xiao（QQ:2811445866）。管理员会视情况对相关人员作出限制进入频道1天、3天、7天的处罚，情节严重的永久踢出本频道。声望错误频道配置等请联系 bossboss wang (qq:304700798)。（打钱尽量备注）● 配置参考 伊斯塔 10/10 通刷所有 热90动95 (can do all escalation)金鹏   Tengu + 中级水晶套(mid-grade crystal set)旗舰    纳迦法 常在河边走 --------------------Transaction ProcessSeller provide linked location, type, remaining time and price, e,g, FH-TTC 10/10 23h 160mThe first man who type 1 in the channel get the site, buyer provide location folder and pay isk to sellerAfter seller receive the isk, seller save the location into buyer's location folderCap Escalation prohibits the exposure of system information on the public channel, and only allows the description of the parachute and outside the parachute, please ask private chatThe bitmark time must be set to permanent; otherwise, you will be banned from the channelSome recommend fittingsThe fit is the same as  above.1.Don't just random type 1 or u will be banned from this channel  2.The resale of location markers is prohibited.".to_string()
    }));
  }

  #[test]
  fn watcher_source_info_parse_testing() {
    let path = "/a/b/c/aaa_bbb_20261018_112434_2124568342.txt";
    let content = r#"
    
    


          ---------------------------------------------------------------

            Channel ID:      player_7e53dc70cae611f1a4b100109bd0f828
            Channel Name:    aaa_bbb
            Listener:        Ecamika
            Session started: 2026.10.18 11:24:34
          ---------------------------------------------------------------

    ﻿[ 2026.10.18 11:24:57 ] Ecamika > 123123456456
    "#;
    let watcher_source_info = WatcherSourceInfo::new(path, content.to_string()).unwrap();
    println!("{:#?}", watcher_source_info);

    assert_eq!(
      watcher_source_info,
      (
        WatcherSourceInfo {
          session_started: utc_datetime!(2026-10-18 11:24:34),
          channel_id: "player_7e53dc70cae611f1a4b100109bd0f828".to_string(),
          channel_name: "aaa_bbb".to_string(),
          listener_id: 2124568342,
          listener: "Ecamika".to_string()
        },
        vec![Message {
          time: utc_datetime!(2026-10-18 11:24:57),
          sender: "Ecamika".to_string(),
          content: "123123456456".to_string()
        }]
      )
    );
  }
}
