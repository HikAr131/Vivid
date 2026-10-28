use eframe::egui;

pub struct Channel {
    pub platform: &'static str,
    pub account: &'static str,
    pub url: &'static str,
}

pub const CHANNELS: &[Channel] = &[
    Channel {
        platform: "抖音",
        account: "Hikari（三角洲行动）",
        url: "https://www.douyin.com/user/MS4wLjABAAAAE1GMl_LBdFUJywgN__Kv1vB9PcgKgwjZ0VbyBSFidfI",
    },
    Channel {
        platform: "B站",
        account: "Hikari三角洲行动",
        url: "https://space.bilibili.com/3690980263856410",
    },
    Channel {
        platform: "小红书",
        account: "Hikari（三角洲行动）",
        url: "https://www.xiaohongshu.com/user/profile/67f9175a000000000a03e7b9",
    },
];

pub const RELEASE_URL: &str = "https://github.com/HikAr131/Vivid/releases/latest";
pub const STREAMLINE_CREDIT_URL: &str = "https://streamlinehq.com";

pub fn allowed(url: &str) -> bool {
    CHANNELS.iter().any(|entry| entry.url == url)
        || url == RELEASE_URL
        || url == STREAMLINE_CREDIT_URL
}

pub fn open(ctx: &egui::Context, url: &str) -> bool {
    if !allowed(url) {
        return false;
    }
    ctx.open_url(egui::OpenUrl::new_tab(url));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_host_allowlist_is_opened() {
        let ctx = egui::Context::default();
        for channel in CHANNELS {
            assert!(open(&ctx, channel.url));
        }
        assert!(open(&ctx, RELEASE_URL));
        assert!(open(&ctx, STREAMLINE_CREDIT_URL));
        for url in [
            "https://example.com",
            "javascript:alert(1)",
            "file:///C:/Windows",
            "https://github.com/HikAr131/Vivid/releases/latest?url=https://evil.com",
            "https://github.com.evil.com/HikAr131/Vivid/releases/latest",
            "https://github.com/HikAr131/other/releases/latest",
            "https://space.bilibili.com/3690980263856410/../1",
            "https://space.bilibili.com/3690980263856410\n",
            "https://streamlinehq.com.evil.com",
            "https://streamlinehq.com?redirect=https://evil.com",
        ] {
            assert!(!open(&ctx, url), "{url}");
        }
        assert_eq!(ctx.output(|o| o.commands.len()), 5);
    }
}
