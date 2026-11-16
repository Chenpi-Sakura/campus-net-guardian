# 校园道路底图

航空港校区与龙泉校区的名称依据[学校校园地图页面](https://www.cuit.edu.cn/ggfw/xydt.htm)核对。道路、建筑、绿地及校园范围来自 OpenStreetMap 公开数据，属于离线示意，不代表实时道路状态或通行许可。

© OpenStreetMap contributors。地图数据库及其衍生数据依 [Open Database License 1.0](https://opendatacommons.org/licenses/odbl/1-0/) 使用，[署名及许可说明](https://www.openstreetmap.org/copyright)。

两张 SVG 与 maps.json 使用相同几何；程序通过 Iced 原生矢量画布绘制底图和路线。sources.json 记录每个校区的原始边界链接、投影原点和米制换算关系。坐标为以校区边界西南角为原点、向东为 x、向北为 y 的局部等距近似投影；这些资源只包含公开地理数据。

内置道路图取校园范围内可步行道路的最大连通分量，另收录航空港校区两条明确标记为 running track 的公开跑道轮廓，来源链接见 sources.json。龙泉校区当前数据缺少跑道轮廓，不能以足球场边界替代。未映射的小路不会被随机补出；参考路径沿已有道路或选定跑道，强扰动可能偏出参考路径，仅用于明确标注的本地模拟。
