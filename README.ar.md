<p align="center"><img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%"></p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>نواة متناهية الصغر. زمن حقيقي جاد. على عتادك.</strong></p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.pt-BR.md">Português</a> · <a href="README.fr.md">Français</a> · <a href="README.de.md">Deutsch</a> · <a href="README.it.md">Italiano</a> · <a href="README.ru.md">Русский</a> · <a href="README.ar.md">العربية</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.id.md">Bahasa Indonesia</a></p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

يجيب NobroRTOS Core عن السؤال الأهم في المنتج المضمّن: **ما المهمة التالية،
وهل يمكنها الانتهاء قبل الموعد؟** تجمع النواة بين تشغيل خفيف بلا heap وقبول
واعٍ بالمواعيد، مع إبقاء التحكم في التعريفات والمقاطعات والمؤقتات والطاقة بيد التطبيق.

## المزايا الأساسية

- **حجم بالغ الصغر:** يبدأ نموذج MCS-51 المقاس من 504 بايت للبرنامج و7 بايت لبيانات النواة.
- **تحقق قبل التشغيل:** يرفض الحمل الزائد والتوقيت غير الصالح والهويات المكررة ومواعيد الاستجابة غير الممكنة.
- **أدوات مرنة:** Arduino وPlatformIO وRust `no_std` وتوليد C/Assembly عبر Python.
- **سلوك محدود وواضح:** سعة ثابتة وأولويات صريحة ومن دون مكدسات مهام مخفية.

## بدء سريع

```text
python -m pip install nobro_rtos
nobro-core ports/byte/examples/useful/app.json --out generated
```

راجع أدلة [Arduino / PlatformIO](docs/ARDUINO_PLATFORMIO.md) و
[Python](docs/PYTHON.md) و[Rust](docs/RUST.md).

## موقع تقني قوي

في نموذج التقييم المرفق الذي يغطي 32 بُعداً و19 نظاماً، يحقق NobroRTOS
**7.23/10 ويأتي في المرتبة الثالثة**. هذا تقييم محدد للمعمارية والقدرات،
وليس حكماً عاماً على جميع الاختبارات أو الاعتمادات أو النضج التجاري.

<p align="center"><img src="docs/images/rtos_pure_technical_ranking.png" alt="الترتيب التقني لأنظمة RTOS" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="تفصيل التقييم التقني" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_radar_comparison.png" alt="مقارنة رادارية لأنظمة RTOS" width="96%"></p>

## نوى NobroRTOS المتقدمة

نوفر أيضاً نوى متقدمة مخصصة لمتطلبات التوقيت والأمان والاتصال والذكاء الاصطناعي
والروبوتات والعتاد. اجتازت هذه النوى اختبار ضغط حقيقياً متزامناً على 18 لوحة تطوير.

<p align="center"><img src="docs/images/real_tests.jpg" alt="اختبار NobroRTOS على 18 لوحة" width="92%"></p>

انضم إلى [Discord](https://discord.gg/NrRrQKmT2) وتابع
[NiusRobotLab على YouTube](https://www.youtube.com/@NiusRobotLab).

ينشر NobroRTOS Core بترخيص **GPL-3.0-only**، والنص المعتمد هو [LICENSE](LICENSE).
