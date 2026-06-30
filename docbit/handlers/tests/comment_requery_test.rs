//! `max_by_key` 回查逻辑的单元测试。
//!
//! rust-ef 1.2.0 的 `save_changes` 不回填自增 id（`on_key_backfill` 以 0 占位，
//! `IAsyncConnection` 无 `last_insert_rowid()`）。`CreateCommentHandler` 的回查策略
//! 是 `linq!(filter by blog_id + user_id).to_list().max_by_key(|c| c.id)`。
//!
//! 本测试验证该回查逻辑在三种场景下的正确性：
//! 1. 同 (blog_id, user_id) 连续插入 → max_by_key 每次返回最新插入
//! 2. 同 blog 不同 user → 各自回查互不干扰
//! 3. 存在软删除评论时 → max_by_key 仍返回最新插入（因新插入 id 最大）

use std::sync::Arc;

use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use rust_ef::prelude::*;
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;

use docbit_domain::entities::{Blog, Category, Comment, User};

/// 构建内存 SQLite DbContext 并建表。
async fn setup_ctx() -> DbContext {
    let mut builder = DbContextOptionsBuilder::new();
    builder.use_sqlite_in_memory();
    let options = Arc::new(builder.build());
    let ctx = DbContext::from_options(&options).expect("Failed to create DbContext");
    // from_options() 已通过 discover_entities() 自动注册所有 #[derive(EntityType)] 实体。
    ctx.ensure_created()
        .await
        .expect("ensure_created failed");
    ctx
}

fn make_category(slug: &str, name: &str) -> Category {
    Category {
        id: 0,
        name: name.into(),
        slug: slug.into(),
        parent_id: None,
        sort_order: 0,
        created_id: None,
        created_at: 0,
        updated_id: None,
        updated_at: 0,
        is_deleted: false,
        parent: BelongsTo::new(),
        children: HasMany::new(),
    }
}

fn make_user(email: &str, name: &str) -> User {
    User {
        id: 0,
        name: name.into(),
        email: email.into(),
        password_hash: "x".into(),
        created_id: None,
        created_at: 0,
        updated_id: None,
        updated_at: 0,
        is_deleted: false,
        roles: HasMany::new(),
    }
}

fn make_blog(slug: &str, title: &str, category_id: i32, author_id: i32) -> Blog {
    Blog {
        id: 0,
        slug: slug.into(),
        title: title.into(),
        summary: String::new(),
        content: "x".into(),
        tags: "[]".into(),
        category_id,
        author_id,
        published_at: 0,
        created_at: 0,
        updated_at: 0,
        created_id: None,
        updated_id: None,
        is_deleted: false,
        category: BelongsTo::new(),
        author: BelongsTo::new(),
        comments: HasMany::new(),
    }
}

fn make_comment(blog_id: i32, user_id: i32, user_name: &str, content: &str) -> Comment {
    Comment {
        id: 0,
        blog_id,
        user_id,
        user_name: user_name.into(),
        content: content.into(),
        parent_id: None,
        quoted_id: None,
        created_at: 0,
        updated_id: None,
        updated_at: 0,
        is_deleted: false,
        blog: BelongsTo::new(),
        user: BelongsTo::new(),
        parent: BelongsTo::new(),
        quoted: BelongsTo::new(),
    }
}

/// 复现 `CreateCommentHandler` 中的回查逻辑：
/// `linq!(filter by blog_id + user_id).to_list().max_by_key(|c| c.id)`
async fn requery_latest(ctx: &mut DbContext, blog_id: i32, user_id: i32) -> Comment {
    linq!(
        ctx.set::<Comment>(),
        |c: Comment| c.blog_id == blog_id && c.user_id == user_id
    )
    .to_list()
    .await
    .expect("requery failed")
    .into_iter()
    .max_by_key(|c| c.id)
    .expect("no comment found after insert")
}

/// 插入 Category 后回查（max_by_key id）。
async fn insert_category(ctx: &mut DbContext, slug: &str, name: &str) -> Category {
    ctx.set::<Category>().add(make_category(slug, name));
    ctx.save_changes().await.unwrap();
    linq!(ctx.set::<Category>(), |c: Category| c.slug == slug)
        .to_list()
        .await
        .unwrap()
        .into_iter()
        .max_by_key(|c| c.id)
        .unwrap()
}

/// 插入 User 后回查。
async fn insert_user(ctx: &mut DbContext, email: &str, name: &str) -> User {
    ctx.set::<User>().add(make_user(email, name));
    ctx.save_changes().await.unwrap();
    linq!(ctx.set::<User>(), |u: User| u.email == email)
        .to_list()
        .await
        .unwrap()
        .into_iter()
        .max_by_key(|u| u.id)
        .unwrap()
}

/// 插入 Blog 后回查。
async fn insert_blog(ctx: &mut DbContext, slug: &str, cat_id: i32, author_id: i32) -> Blog {
    ctx.set::<Blog>().add(make_blog(slug, "Blog", cat_id, author_id));
    ctx.save_changes().await.unwrap();
    linq!(ctx.set::<Blog>(), |b: Blog| b.slug == slug)
        .to_list()
        .await
        .unwrap()
        .into_iter()
        .max_by_key(|b| b.id)
        .unwrap()
}

#[tokio::test]
async fn max_by_key_returns_latest_inserted() {
    let mut ctx = setup_ctx().await;

    let cat = insert_category(&mut ctx, "uncat", "未分类").await;
    let user = insert_user(&mut ctx, "u1@x.com", "U1").await;
    let blog = insert_blog(&mut ctx, "b1", cat.id, user.id).await;

    // 同 (blog_id, user_id) 连续插入 3 条评论，每次回查验证 max_by_key 返回最新。
    let mut ids = Vec::new();
    for i in 0..3 {
        let c = make_comment(blog.id, user.id, "U1", &format!("comment-{}", i));
        ctx.set::<Comment>().add(c);
        ctx.save_changes().await.unwrap();
        let last = requery_latest(&mut ctx, blog.id, user.id).await;
        ids.push(last.id);
    }

    // id 严格递增：每次 max_by_key 返回的是最新插入的那条。
    assert!(
        ids.windows(2).all(|w| w[1] > w[0]),
        "ids should be strictly increasing: {:?}",
        ids
    );

    // 最终回查返回最后一条评论。
    let final_last = requery_latest(&mut ctx, blog.id, user.id).await;
    assert_eq!(final_last.id, *ids.last().unwrap());
    assert_eq!(final_last.content, "comment-2");
}

#[tokio::test]
async fn max_by_key_isolated_per_blog_user_pair() {
    let mut ctx = setup_ctx().await;

    let cat = insert_category(&mut ctx, "uncat", "未分类").await;
    let u1 = insert_user(&mut ctx, "u1@x.com", "U1").await;
    let u2 = insert_user(&mut ctx, "u2@x.com", "U2").await;
    let blog = insert_blog(&mut ctx, "b1", cat.id, u1.id).await;

    // U1 插入第一条评论
    ctx.set::<Comment>()
        .add(make_comment(blog.id, u1.id, "U1", "hello from u1"));
    ctx.save_changes().await.unwrap();

    // U2 插入评论（同 blog 不同 user）
    ctx.set::<Comment>()
        .add(make_comment(blog.id, u2.id, "U2", "hello from u2"));
    ctx.save_changes().await.unwrap();

    // U1 再插入一条（此时 DB 中有 3 条评论，U1 两条 + U2 一条）
    ctx.set::<Comment>()
        .add(make_comment(blog.id, u1.id, "U1", "second from u1"));
    ctx.save_changes().await.unwrap();

    let u1_last = requery_latest(&mut ctx, blog.id, u1.id).await;
    let u2_last = requery_latest(&mut ctx, blog.id, u2.id).await;

    // U1 回查返回 U1 的第二条（不是 U2 的）
    assert_eq!(u1_last.content, "second from u1");
    assert_eq!(u1_last.user_id, u1.id);

    // U2 回查返回 U2 的唯一一条（不是 U1 的）
    assert_eq!(u2_last.content, "hello from u2");
    assert_eq!(u2_last.user_id, u2.id);

    // U1 第二条是最后插入的，id 最大；U2 的 id 小于 U1 最后一条。
    assert!(u2_last.id < u1_last.id);
}

#[tokio::test]
async fn max_by_key_correct_among_soft_deleted() {
    let mut ctx = setup_ctx().await;

    let cat = insert_category(&mut ctx, "uncat", "未分类").await;
    let user = insert_user(&mut ctx, "u1@x.com", "U1").await;
    let blog = insert_blog(&mut ctx, "b1", cat.id, user.id).await;

    // 先插入一条评论，然后软删除它
    ctx.set::<Comment>()
        .add(make_comment(blog.id, user.id, "U1", "old soft-deleted"));
    ctx.save_changes().await.unwrap();
    let mut c1 = requery_latest(&mut ctx, blog.id, user.id).await;
    c1.is_deleted = true;
    ctx.set::<Comment>().update(c1);
    ctx.save_changes().await.unwrap();

    // 再插入一条新评论（未被软删除）
    ctx.set::<Comment>()
        .add(make_comment(blog.id, user.id, "U1", "new active"));
    ctx.save_changes().await.unwrap();

    // 回查：max_by_key(c.id) 应返回新插入的那条（id 更大），而非软删除的。
    // 注意：回查的 linq! 谓词只过滤 blog_id + user_id，不过滤 is_deleted
    // （与 CreateCommentHandler 一致）。新插入的 id 最大，所以 max_by_key 正确返回它。
    let last = requery_latest(&mut ctx, blog.id, user.id).await;
    assert_eq!(last.content, "new active");
    assert!(
        !last.is_deleted,
        "max_by_key should return the newly inserted comment, not the soft-deleted one"
    );
}
