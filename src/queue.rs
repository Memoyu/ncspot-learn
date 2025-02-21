use std::cmp::Ordering;
use std::sync::{Arc, RwLock};

use log::{debug, info};
#[cfg(feature = "notify")]
use notify_rust::Notification;

use rand::prelude::*;
use strum_macros::Display;

use crate::config::Config;
use crate::library::Library;
use crate::model::playable::Playable;
use crate::spotify::PlayerEvent;
use crate::spotify::Spotify;

/// Repeat behavior for the [Queue].
/// 循环枚举
#[derive(Display, Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum RepeatSetting {
    #[serde(rename = "off")]
    None, // 不循环
    #[serde(rename = "playlist")]
    RepeatPlaylist, // 循环播放列表
    #[serde(rename = "track")]
    RepeatTrack, // 循环单曲
}

/// Events that are specific to the [Queue].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueEvent {
    /// Request the player to 'preload' a track, basically making sure that
    /// transitions between tracks can be uninterrupted.
    PreloadTrackRequest,
}

/// The queue determines the playback order of [Playable] items, and is also used to control
/// playback itself.
/// 播放歌曲列表，同时控制播放器
pub struct Queue {
    /// The internal data, which doesn't change with shuffle or repeat. This is
    /// the raw data only.
    /// 播放列表，原始数据
    pub queue: Arc<RwLock<Vec<Playable>>>,
    /// The playback order of the queue, as indices into `self.queue`.
    /// 播放列表播放顺序，存储queue的索引
    random_order: RwLock<Option<Vec<usize>>>,
    /// 当前播放的歌曲，queue的索引
    current_track: RwLock<Option<usize>>,
    /// Spotify实例
    spotify: Spotify,
    /// 配置实例
    cfg: Arc<Config>,
    /// library实例
    library: Arc<Library>,
}

impl Queue {
    pub fn new(spotify: Spotify, cfg: Arc<Config>, library: Arc<Library>) -> Self {
        // 获取播放列表状态缓存
        let queue_state = cfg.state().queuestate.clone();

        Self {
            queue: Arc::new(RwLock::new(queue_state.queue)),
            spotify: spotify.clone(),
            current_track: RwLock::new(queue_state.current_track),
            random_order: RwLock::new(queue_state.random_order),
            cfg,
            library,
        }
    }

    /// The index of the next item in `self.queue` that should be played. None
    /// if at the end of the queue.
    /// 获取下一首歌曲的索引，如果是队列的最后一首，则返回None
    pub fn next_index(&self) -> Option<usize> {
        match *self.current_track.read().unwrap() {
            Some(mut index) => {
                let random_order = self.random_order.read().unwrap();
                // 如果随机播放列表不为空
                if let Some(order) = random_order.as_ref() {
                    // 获取当前播放歌曲索引（queue对应索引）在random_order中对应的索引
                    index = order.iter().position(|&i| i == index).unwrap();
                }

                let mut next_index = index + 1;
                // 索引大于队列长度时，则返回None
                if next_index < self.queue.read().unwrap().len() {
                    // 如果随机播放列表不为空
                    if let Some(order) = random_order.as_ref() {
                        // 获取随机播放列表对应索引的值（queue对应索引）
                        next_index = order[next_index];
                    }

                    // queue对应索引
                    Some(next_index)
                } else {
                    None
                }
            }
            None => None,
        }
    }

    /// The index of the previous item in `self.queue` that should be played.
    /// None if at the start of the queue.
    /// 获取上一首歌曲的索引，如果是队列的第一首，则返回None
    pub fn previous_index(&self) -> Option<usize> {
        match *self.current_track.read().unwrap() {
            Some(mut index) => {
                let random_order = self.random_order.read().unwrap();
                if let Some(order) = random_order.as_ref() {
                    index = order.iter().position(|&i| i == index).unwrap();
                }

                if index > 0 {
                    let mut next_index = index - 1;
                    if let Some(order) = random_order.as_ref() {
                        next_index = order[next_index];
                    }

                    Some(next_index)
                } else {
                    None
                }
            }
            None => None,
        }
    }

    /// The currently playing item from `self.queue`.
    /// 获取当前播放的歌曲
    pub fn get_current(&self) -> Option<Playable> {
        self.get_current_index()
            .map(|index| self.queue.read().unwrap()[index].clone())
    }

    /// The index of the currently playing item from `self.queue`.
    /// 获取当前播放的歌曲的索引
    pub fn get_current_index(&self) -> Option<usize> {
        *self.current_track.read().unwrap()
    }

    /// Insert `track` as the item that should logically follow the currently
    /// playing item, taking into account shuffle status.
    /// 插入歌曲到当前播放歌曲后面
    pub fn insert_after_current(&self, track: Playable) {
        if let Some(index) = self.get_current_index() {
            let mut random_order = self.random_order.write().unwrap();
            if let Some(order) = random_order.as_mut() {
                // 更新随机播放列表中的queue索引
                let next_i = order.iter().position(|&i| i == index).unwrap();
                // shift everything after the insertion in order
                // 对随机播放列表中大于current_track index的queue索引进行加1
                for item in order.iter_mut() {
                    if *item > index {
                        *item += 1;
                    }
                }
                // finally, add the next track index
                // 再插入下一首歌曲的索引到random_order中
                order.insert(next_i + 1, index + 1);
            }

            // 再插入下一首歌曲到queue中
            let mut q = self.queue.write().unwrap();
            q.insert(index + 1, track);
        } else {
            // 没有当前播放歌曲，则插入到队列的末尾
            self.append(track);
        }
    }

    /// Add `track` to the end of the queue.
    /// 将歌曲插入到队列的末尾
    pub fn append(&self, track: Playable) {
        let mut random_order = self.random_order.write().unwrap();
        if let Some(order) = random_order.as_mut() {
            // 饱和减法
            // 出现溢出时，不发生报错，返回最小值，usize最小值为0
            // random_order长度与queue长度一致，当前index为queue最后一个元素的下标
            let index = order.len().saturating_sub(1);
            order.push(index);
        }

        let mut q = self.queue.write().unwrap();
        q.push(track);
    }

    /// Append `tracks` after the currently playing item, taking into account
    /// shuffle status. Returns the amount of added items.
    /// 将歌曲插入到当前曲目之后，考虑顺序播放状态。返回添加的曲目第一条的索引
    pub fn append_next(&self, tracks: &Vec<Playable>) -> usize {
        let mut q = self.queue.write().unwrap();

        {
            let mut random_order = self.random_order.write().unwrap();
            if let Some(order) = random_order.as_mut() {
                order.extend((q.len().saturating_sub(1))..(q.len() + tracks.len()));
            }
        }

        let first = match *self.current_track.read().unwrap() {
            Some(index) => index + 1,
            None => q.len(),
        };

        let mut i = first;
        for track in tracks {
            q.insert(i, track.clone());
            i += 1;
        }

        first
    }

    /// Remove the item at `index`. This doesn't take into account shuffle
    /// status, and will literally remove the item at `index` in `self.queue`.
    /// 删除指定索引的曲目, 不考虑随机播放, 直接删除self.queue索引
    pub fn remove(&self, index: usize) {
        {
            let mut q = self.queue.write().unwrap();
            if q.len() == 0 {
                info!("queue is empty");
                return;
            }
            q.remove(index);
        }

        // if the queue is empty stop playback
        // 删除完了再看看，空的话就停止播放
        let len = self.queue.read().unwrap().len();
        if len == 0 {
            self.stop();
            return;
        }

        // if we are deleting the currently playing track, play the track with
        // the same index again, because the next track is now at the position
        // of the one we deleted
        let current = *self.current_track.read().unwrap();
        if let Some(current_track) = current {
            // 比对当前播放的索引和删除的索引
            match current_track.cmp(&index) {
                // current_track = index 相等
                Ordering::Equal => {
                    // if we have deleted the last item and it was playing
                    // stop playback, unless repeat playlist is on, play next
                    // 如果是列表最后一首歌曲
                    if current_track == len {
                        // 如果循环模式为列表循环
                        if self.get_repeat() == RepeatSetting::RepeatPlaylist {
                            // 播放下一首
                            self.next(false);
                        } else {
                            // 否则，停止播放
                            self.stop();
                        }
                    } else {
                        // queue下指定index的各歌曲已被删除，此时index对应的是新的一首歌曲
                        // 继续播放这首新的歌曲
                        self.play(index, false, false);
                    }
                }
                // current_track > index 大于
                Ordering::Greater => {
                    // 向前移动一位
                    // 索引减少1
                    let mut current = self.current_track.write().unwrap();
                    current.replace(current_track - 1);
                }
                _ => (),
            }
        }

        // 如果随机播放，则重新生成播放顺序
        if self.get_shuffle() {
            self.generate_random_order();
        }
    }

    /// Clear all the items from the queue and stop playback.
    /// 清空列表项，并停止播放
    pub fn clear(&self) {
        self.stop();

        let mut q = self.queue.write().unwrap();
        q.clear();

        // 清空随机列表
        let mut random_order = self.random_order.write().unwrap();
        if let Some(o) = random_order.as_mut() {
            o.clear()
        }
    }

    /// The amount of items in `self.queue`.
    /// queue列表歌曲数量
    pub fn len(&self) -> usize {
        self.queue.read().unwrap().len()
    }

    /// Shift the item at `from` in `self.queue` to `to`.
    /// 将queue中的from项移动到to位置
    pub fn shift(&self, from: usize, to: usize) {
        let mut queue = self.queue.write().unwrap();
        // 先移除
        let item = queue.remove(from);
        // 再插入
        queue.insert(to, item);

        // if the currently playing track is affected by the shift, update its
        // index
        // 如果当前播放索引被移动影响了，更新索引
        let mut current = self.current_track.write().unwrap();
        if let Some(index) = *current {
            if index == from {
                current.replace(to);
            } else if index == to && from > index {
                current.replace(to + 1);
            } else if index == to && from < index {
                current.replace(to - 1);
            }
        }
    }

    /// Play the item at `index` in `self.queue`.
    ///
    /// `reshuffle`: Reshuffle the current order of the queue.
    /// `shuffle_index`: If this is true, `index` isn't actually used, but is
    /// chosen at random as a valid index in the queue.
    ///
    /// 播放指定index的歌曲
    ///
    /// reshuffle: 重新生成随机播放顺序
    /// shuffle_index: 使用随机生成索引 如果为true,则实际上index不会使用，而是随机选取queue的索引
    pub fn play(&self, mut index: usize, reshuffle: bool, shuffle_index: bool) {
        let queue_length = self.queue.read().unwrap().len();
        // The length of the queue must be bigger than 0 or gen_range panics!
        // 队列长度必须大于0，否者程序会panics
        // 生成随机索引
        if queue_length > 0 && shuffle_index && self.get_shuffle() {
            let mut rng = rand::thread_rng();
            index = rng.gen_range(0..queue_length);
        }

        if let Some(track) = &self.queue.read().unwrap().get(index) {
            self.spotify.load(track, true, 0);
            // 替换当前播放索引
            let mut current = self.current_track.write().unwrap();
            current.replace(index);
            //
            self.spotify.update_track();

            #[cfg(feature = "notify")]
            if self.cfg.values().notify.unwrap_or(false) {
                std::thread::spawn({
                    // use same parser as track_format, Playable::format
                    // 获取配置的通知格式
                    let format = self
                        .cfg
                        .values()
                        .notification_format
                        .clone()
                        .unwrap_or_default();
                    let default_title = crate::config::NotificationFormat::default().title.unwrap();
                    let title = format.title.unwrap_or_else(|| default_title.clone());

                    let default_body = crate::config::NotificationFormat::default().body.unwrap();
                    let body = format.body.unwrap_or_else(|| default_body.clone());

                    // 根据模板生成文本
                    let summary_txt = Playable::format(track, &title, &self.library);
                    let body_txt = Playable::format(track, &body, &self.library);
                    let cover_url = track.cover_url();
                    move || send_notification(&summary_txt, &body_txt, cover_url)
                });
            }

            // Send a Seeked signal at start of new track
            #[cfg(feature = "mpris")]
            self.spotify.notify_seeked(0);
        }

        if reshuffle && self.get_shuffle() {
            self.generate_random_order()
        }
    }

    /// Toggle the playback. If playback is currently stopped, this will either
    /// play the next song if one is available, or restart from the start.
    /// 切换播放状态（播放/暂停）
    pub fn toggleplayback(&self) {
        match self.spotify.get_current_status() {
            // 当前播放状态为播放/暂停，则切换播放状态
            PlayerEvent::Playing(_) | PlayerEvent::Paused(_) => {
                self.spotify.toggleplayback();
            }
            // 当前状态状态为停止，则播放下一首或播放第一首
            PlayerEvent::Stopped => match self.next_index() {
                Some(_) => self.next(false),
                None => self.play(0, false, false),
            },
            _ => (),
        }
    }

    /// Stop playback.
    /// 停止播放
    pub fn stop(&self) {
        let mut current = self.current_track.write().unwrap();
        *current = None;
        self.spotify.stop();
    }

    /// Play the next song in the queue.
    ///
    /// `manual`: If this is true, normal queue logic like repeat will not be
    /// used, and the next track will actually be played. This should be used
    /// when going to the next entry in the queue is the wanted behavior.
    /// 播放下一首
    pub fn next(&self, manual: bool) {
        let q = self.queue.read().unwrap();
        let current = *self.current_track.read().unwrap();
        let repeat = self.cfg.state().repeat;

        // 如果当前循环模式为单曲循环，且不是手动切歌
        if repeat == RepeatSetting::RepeatTrack && !manual {
            // 继续重新播放当前歌曲
            if let Some(index) = current {
                self.play(index, false, false);
            }
        } else if let Some(index) = self.next_index() {
            self.play(index, false, false);

            // 如果当前循环模式为单曲循环，且为手动切歌
            if repeat == RepeatSetting::RepeatTrack && manual {
                // 将循环模式配置为列表循环
                self.set_repeat(RepeatSetting::RepeatPlaylist);
            }
        } else if repeat == RepeatSetting::RepeatPlaylist && q.len() > 0 {
            // 如果上述条件都不符合，且循环模式为列表循环、队列部位空时
            // 则获取random_order列表的第一个
            // 不存在时则播放queue列表的第一个

            let random_order = self.random_order.read().unwrap();
            self.play(
                random_order.as_ref().map(|o| o[0]).unwrap_or(0),
                false,
                false,
            );
        } else {
            // 否则停止播放
            self.spotify.stop();
        }
    }

    /// Play the previous item in the queue.
    /// 播放上一首
    pub fn previous(&self) {
        let q = self.queue.read().unwrap();
        let current = *self.current_track.read().unwrap();
        let repeat = self.cfg.state().repeat;

        if let Some(index) = self.previous_index() {
            self.play(index, false, false);
        } else if repeat == RepeatSetting::RepeatPlaylist && q.len() > 0 {
            if self.get_shuffle() {
                let random_order = self.random_order.read().unwrap();
                self.play(
                    random_order.as_ref().map(|o| o[q.len() - 1]).unwrap_or(0),
                    false,
                    false,
                );
            } else {
                self.play(q.len() - 1, false, false);
            }
        } else if let Some(index) = current {
            self.play(index, false, false);
        }
    }

    /// Get the current repeat behavior.
    /// 获取当前配置的循环状态
    pub fn get_repeat(&self) -> RepeatSetting {
        self.cfg.state().repeat
    }

    /// Set the current repeat behavior and save it to the configuration.
    /// 设置当前配置的循环状态
    pub fn set_repeat(&self, new: RepeatSetting) {
        self.cfg.with_state_mut(|s| s.repeat = new);
    }

    /// Get the current shuffle behavior.
    /// 获取当前配置的随机播放状态
    pub fn get_shuffle(&self) -> bool {
        self.cfg.state().shuffle
    }

    /// Get the current order that is used to shuffle.
    /// 获取随机播放列表
    pub fn get_random_order(&self) -> Option<Vec<usize>> {
        self.random_order.read().unwrap().clone()
    }

    /// (Re)generate the random shuffle order.
    /// 生成随机播放顺序
    fn generate_random_order(&self) {
        let q = self.queue.read().unwrap();
        let mut order: Vec<usize> = Vec::with_capacity(q.len());
        let mut random: Vec<usize> = (0..q.len()).collect();

        if let Some(current) = *self.current_track.read().unwrap() {
            order.push(current);
            random.remove(current);
        }

        let mut rng = rand::thread_rng();
        // 将可变切片原地打乱
        random.shuffle(&mut rng);
        // 追加随机顺序
        order.extend(random);

        let mut random_order = self.random_order.write().unwrap();
        *random_order = Some(order);
    }

    /// Set the current shuffle behavior.
    /// 设置随机播放状态
    pub fn set_shuffle(&self, new: bool) {
        self.cfg.with_state_mut(|s| s.shuffle = new);
        if new {
            self.generate_random_order();
        } else {
            let mut random_order = self.random_order.write().unwrap();
            *random_order = None;
        }
    }

    /// Handle events that are specific to the queue.
    /// 处理指定的队列事件
    pub fn handle_event(&self, event: QueueEvent) {
        match event {
            QueueEvent::PreloadTrackRequest => {
                if let Some(next_index) = self.next_index() {
                    let track = self.queue.read().unwrap()[next_index].clone();
                    debug!("Preloading track {} as requested by librespot", track);
                    self.spotify.preload(&track);
                }
            }
        }
    }

    /// Get the spotify session.
    pub fn get_spotify(&self) -> Spotify {
        self.spotify.clone()
    }
}

/// Send a notification using the desktops default notification method.
///
/// `summary_txt`: A short title for the notification.
/// `body_txt`: The actual content of the notification.
/// `cover_url`: A URL to an image to show in the notification.
/// `notification_id`: Unique id for a notification, that can be used to operate
/// on a previous notification (for example to close it).
#[cfg(feature = "notify")]
pub fn send_notification(summary_txt: &str, body_txt: &str, cover_url: Option<String>) {
    let mut n = Notification::new();
    n.appname("ncspot").summary(summary_txt).body(body_txt);

    // album cover image
    if let Some(u) = cover_url {
        let path = crate::utils::cache_path_for_url(u.to_string());
        if !path.exists() {
            if let Err(e) = crate::utils::download(u, path.clone()) {
                log::error!("Failed to download cover: {}", e);
            }
        }
        n.icon(path.to_str().unwrap());
    }

    // XDG desktop entry hints
    #[cfg(all(unix, not(target_os = "macos")))]
    n.urgency(notify_rust::Urgency::Low)
        .hint(notify_rust::Hint::Transient(true))
        .hint(notify_rust::Hint::DesktopEntry("ncspot".into()));

    match n.show() {
        Ok(handle) => {
            // only available for XDG
            #[cfg(all(unix, not(target_os = "macos")))]
            info!("Created notification: {}", handle.id());
        }
        Err(e) => log::error!("Failed to send notification cover: {}", e),
    }
}
