//! 手动验证更新检查的真实链路（DNS + TLS + 解析）。
//!
//! ```text
//! cargo run -p vivid-core --example check_update                     # 用默认地址与真实版本
//! cargo run -p vivid-core --example check_update -- <URL>            # 指定地址
//! cargo run -p vivid-core --example check_update -- <URL> 0.0.1      # 假装本地是旧版
//! ```
//!
//! 第二个参数是用来**验证「有更新」那条分支**的。发布当天清单版本通常等于
//! 本地版本，只跑得到「已是最新」；而用户真正收到更新时走的是另一条路径，
//! 不假装一次旧版，那条路径到上线都没被执行过。
//!
//! 单元测试只覆盖版本比较和清单解析——那些是纯逻辑。
//! 网络这一段只有真跑一次才知道 TLS 配置对不对，
//! 而「编译通过」跟「能连上 https」是两回事。

use vivid_core::update::{self, DEFAULT_MANIFEST_URL};

fn main() {
    // filter 掉空串：想「用默认地址但指定版本」时会传 "" 占位，
    // 只判 None 的话会拿着空 URL 去请求，报一个莫名其妙的 empty string。
    let url = std::env::args()
        .nth(1)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MANIFEST_URL.to_string());
    // 可选：假装本地是某个旧版本，用来跑通「有更新」那条分支。
    let local = std::env::args()
        .nth(2)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| update::current_version().to_string());

    println!("\n当前版本：{local}");
    println!("清单地址：{url}\n");

    let t = std::time::Instant::now();
    let result = update::check(&url, &local);
    let ms = t.elapsed().as_millis();

    match result {
        Ok(Some(a)) => {
            println!("[有更新] {} → {}", a.current, a.manifest.version);
            if !a.manifest.notes.is_empty() {
                println!("  说明：{}", a.manifest.notes);
            }
            if !a.manifest.download_url.is_empty() {
                println!("  下载：{}", a.manifest.download_url);
            }
        }
        Ok(None) => println!("[已是最新] 远端版本不高于本地"),
        Err(e) => println!("[失败] {e}"),
    }

    println!("\n耗时 {ms} ms");
    println!("（失败也是正常结果之一：清单还没上传时就该是 404。");
    println!("  关键是看它有没有在超时内给出明确答复，而不是卡住或 panic。）\n");
}
